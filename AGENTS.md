# Agent instructions

This file is for AI coding agents working in this repository. Human
contributors should start with [CONTRIBUTING.md](CONTRIBUTING.md).

## Read first

- [CONTRIBUTING.md](CONTRIBUTING.md) — setup, quality gates, and conventions.
- [docs/README.md](docs/README.md) — user and architecture documentation.
- [docs/architecture/README.md](docs/architecture/README.md) — which document is
  normative and which is a historical report.

## Repository rules

- **Do not commit generated or local state:** `target/`, `.lynx/`, `.lea/`,
  `.fastembed_cache/`, `benchmarks/reports/`, model weights, or `.env` files.
- **Keep the layer boundary.** Product logic belongs in `lynx-core`; persistence
  in `lynx-storage`; transports (`lynx-cli`, `lynx-mcp`) contain no product
  logic.
- **Treat `lynx-protocol` serialization as frozen.** Adding a serialized field
  is a wire change; renaming or removing one is breaking. Update
  `crates/lynx-protocol/tests/wire_contract.rs` and docs together.
- **Relations are intra-file.** Do not add cross-file graph linkage without a
  design discussion.
- **Preserve determinism and bounds.** No hash-dependent ranking, no
  repository-proportional query memory, no unbounded candidate sets.
- **No telemetry, network listeners, or silent network calls.** The embedding
  model download on first use is the only expected network access.
- **Do not add a dependency** without a clear need and a note in the pull
  request. Prefer the existing stack.

## Before finishing a change

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 scripts/docs/check_links.py   # documentation changes
```

For retrieval or performance work, add a grounded benchmark task and report
before/after numbers. For distribution changes, run the relevant script under
`tests/distribution/`.

## Using Lynx on itself

If the repository has a fresh `.lynx` index, prefer it over ad-hoc grep for
symbol discovery:

```bash
lx search "query"
lx resolve SymbolName
lx relations SymbolName
```

The index is local state and is never committed. If it is missing, run
`lx index` or use ordinary source reads; do not commit the result.
