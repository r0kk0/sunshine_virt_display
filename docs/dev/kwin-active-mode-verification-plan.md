# KWin Active Mode Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Verify the physical KWin mode instead of scaled logical geometry after connecting a virtual display.

**Architecture:** Extend the existing KScreen text parser with an internal active-mode lookup keyed by validated connector name. Keep layout geometry unchanged, use the active mode for final verification, and split output-detection and mode-verification errors.

**Tech Stack:** Rust stable, `kscreen-doctor`, existing `svd-daemon` unit tests.

---

### Task 1: Parse and match the active KScreen mode

**Files:**
- Modify: `crates/svd-daemon/src/strategy/kwin/kscreen.rs`

- [ ] **Step 1: Write failing parser tests**

Add tests for a scaled output whose `Modes:` line contains a preferred `!` mode and an active `*` mode. Assert that `active_mode_matches(text, "DP-3", 3840, 2160, 144)` succeeds despite `Geometry: 2560x1440`, that 59.94 matches requested 60, and that wrong dimensions, wrong refresh, preferred-only markers, malformed tokens, disabled outputs, and wrong connectors fail.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p svd-daemon active_mode -- --nocapture
```

Expected: compilation fails because `active_mode_matches` does not exist.

- [ ] **Step 3: Implement the minimal parser**

Add an internal `ActiveMode { width: u32, height: u32, refresh: f64 }`, parse only the `Modes:` token ending in `*` inside the requested validated output block, and expose:

```rust
pub(crate) fn active_mode_matches(
    text: &str,
    connector: &str,
    width: u32,
    height: u32,
    refresh: u32,
) -> bool
```

Require the output to be enabled and compare refresh with `abs(active - requested) <= 0.5`.

- [ ] **Step 4: Run focused tests and verify GREEN**

```bash
cargo test -p svd-daemon active_mode -- --nocapture
```

Expected: all active-mode tests pass.

### Task 2: Use active-mode verification and improve errors

**Files:**
- Modify: `crates/svd-daemon/src/strategy/mod.rs`
- Modify: `crates/svd-daemon/src/strategy/kwin/mod.rs`

- [ ] **Step 1: Add distinct strategy errors**

Replace `Timeout` with:

```rust
#[error("connect timeout waiting for KWin to detect the virtual output")]
OutputDetectionTimeout,
#[error("KWin did not apply the requested virtual display mode")]
ModeVerificationFailed,
```

- [ ] **Step 2: Replace verification behavior**

Use `OutputDetectionTimeout` only after the polling deadline. After `run_twice`, read `kscreen-doctor -o` once and call `active_mode_matches`; return `ModeVerificationFailed` when it returns false. Do not compare requested pixels to `OutputInfo` geometry.

- [ ] **Step 3: Run daemon tests**

```bash
cargo test -p svd-daemon
```

Expected: all daemon tests pass.

### Task 3: Full verification and commit

**Files:**
- Verify all modified Rust files and the approved design/plan documents.

- [ ] **Step 1: Run repository checks**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all commands exit 0 without warnings.

- [ ] **Step 2: Commit only task files**

```bash
git add crates/svd-daemon/src/strategy/kwin/kscreen.rs \
  crates/svd-daemon/src/strategy/kwin/mod.rs \
  crates/svd-daemon/src/strategy/mod.rs \
  docs/dev/kwin-active-mode-verification-plan.md
git commit -m "fix(kwin): verify active display mode"
```

- [ ] **Step 3: Hardware acceptance**

Install and restart the daemon, then run `svd connect --width 1920 --height 1080 --refresh 60`, `svd status`, and `svd disconnect`. Expected: connect succeeds on a scaled KWin session and cleanup returns to disconnected state.
