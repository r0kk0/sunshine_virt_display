# Repository Guidelines

## Project Structure & Module Organization

The active implementation is a Rust workspace. `crates/svd-daemon/` contains the privileged daemon and KWin display strategy, `crates/svd-cli/` provides the `svd` client, and `crates/svd-proto/` defines shared IPC messages and framing. Rust integration tests live under each crate's `tests/` directory.

The earlier Python implementation remains in `src/`, with its entry point at `main.py` and tests in `tests/`. Deployment files are under `deploy/`; `install.sh` builds and installs the Rust binaries and systemd unit. Keep operational documentation in `readme.md` and developer notes in `docs/dev/`.

## Build, Test, and Development Commands

- `cargo build --workspace`: compile all Rust crates in debug mode.
- `cargo test --workspace`: run Rust unit and integration tests.
- `cargo run --bin svd -- --help`: smoke-test the CLI locally.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --all-targets`: catch common Rust issues.
- `pytest --cov=src --cov=main --cov-report=term-missing`: run the legacy Python suite with coverage.
- `sudo ./install.sh`: build release binaries and install the production service.
- `make dev-install` / `make dev-logs`: install the Python development service and follow its journal.

Commands that manipulate displays or systemd require Linux, KWin/Wayland, and appropriate root access. Prefer unit tests and mocked filesystem/process boundaries during routine development.

## Coding Style & Naming Conventions

Use standard `rustfmt` output and idiomatic Rust naming: `snake_case` for modules and functions, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep protocol types in `svd-proto`; avoid duplicating validation or wire formats across binaries. Python uses four-space indentation, `snake_case`, type hints where practical, and `test_<behavior>` test names.

## Testing Guidelines

Add regression tests with every behavioral fix. Rust unit tests should stay near the implementation; end-to-end CLI or daemon behavior belongs in crate-level `tests/`. Python tests use `pytest` and mocks to avoid real DRM, sysfs, and compositor changes. No fixed coverage threshold is enforced, but CI reports missing Python coverage.

## Commit & Pull Request Guidelines

History follows Conventional Commits with optional scopes, for example `feat(svd-daemon): add restore guard` or `fix(svd-cli): validate device`. Keep commits focused. Pull requests should explain behavior and risk, list verification commands, link relevant issues, and include logs or screenshots when display behavior changes. Call out root/systemd requirements and recovery steps for changes touching DRM, KWin, or installation.
