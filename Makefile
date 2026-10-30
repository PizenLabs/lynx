.PHONY: fmt fmt-check check clippy test ci dist-test docs

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

check:
	cargo check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

docs:
	python3 scripts/docs/check_links.py

# Installer behavior + clean-machine install/index/query/MCP flow.
dist-test:
	cargo build --release -p pizen-lynx
	tests/distribution/run.sh

ci: fmt-check check clippy test
