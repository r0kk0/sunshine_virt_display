.PHONY: build test lint check install

build:
	cargo build --workspace

test:
	cargo test --workspace

lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

check: lint test

install:
	@test -n "$(USER)" || (echo "USER is required" >&2; exit 2)
	sudo ./install.sh --user "$(USER)"
