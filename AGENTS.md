# Repository Guidelines

## Project Structure & Module Organization

This is a Rust workspace. `crates/svd-daemon/` contains privileged IPC, lifecycle, KWin, DRM/sysfs, sleep, and Sunshine watcher logic. `crates/svd-cli/` provides the unprivileged `svd` command, while `crates/svd-proto/` owns shared wire types, validation, and framing. Unit tests stay beside their modules; binary and socket integration tests live in each crate's `tests/` directory.

Deployment files are under `deploy/`. `install.sh` builds and installs the binaries and systemd unit. `scripts/debug_virt_display.py` is a standalone diagnostic utility, not a runtime implementation. Keep operational documentation in `readme.md` and developer notes in `docs/dev/`.

## Build, Test, and Development Commands

- `cargo build --workspace`: compile all crates in debug mode.
- `cargo test --workspace`: run unit, CLI, protocol, and IPC integration tests.
- `cargo fmt --all -- --check`: verify canonical Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: enforce warning-free idiomatic Rust.
- `cargo run --bin svd -- --help`: smoke-test the CLI.
- `make check`: run formatting, Clippy, and all tests.
- `sudo ./install.sh --user "$USER"`: build release binaries and install for the desktop user.

Commands that manipulate displays or systemd require Linux, KWin/Wayland, and appropriate privileges. Routine tests must use temporary sockets, fixtures, or injected boundaries rather than real DRM state.

## Coding Style & Naming Conventions

Use `rustfmt` output and idiomatic naming: `snake_case` for modules/functions, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep wire types in `svd-proto`. Validate external identifiers at the boundary and carry `CardId`/`ConnectorId` internally instead of raw path fragments. Avoid `unwrap`/`expect` in daemon runtime paths.

## Testing Guidelines

Use red-green-refactor for behavioral changes. Add regression tests for malformed IPC, authorization, recovery phases, partial failures, and idempotent cleanup. Keep hardware-dependent verification documented separately for Intel, AMD, and NVIDIA KWin systems.

## Commit & Pull Request Guidelines

Use focused Conventional Commits, such as `security(ipc): bound client reads` or `fix(strategy): preserve recovery journal`. Commit bodies should state `Why`, `Behavior`, and `Verification`. Pull requests must explain risk, migration impact, test commands, and recovery steps for DRM, KWin, installer, or systemd changes.
