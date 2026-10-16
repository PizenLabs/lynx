.PHONY: fmt check clippy test ci dist-test

fmt:
	cargo fmt --all

check:
	cargo check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

# Installer behavior + clean-machine install/index/query/MCP flow.
dist-test:
	cargo build --release -p pizen-lynx
	tests/distribution/run.sh

ci: fmt check clippy test
