# Contributing to Lynx

Thanks for taking the time to contribute. Lynx is a small, focused project:
a deterministic repository-intelligence engine that ships one binary (`lx`) and
one stdio MCP server. The best contributions are narrowly scoped, measured, and
documented.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
For security issues, **do not open a public issue** — see [SECURITY.md](SECURITY.md).

## Ways to contribute

- **Bug reports** — use the bug report template and include `lx --version`,
  `lx doctor`, and a minimal reproduction.
- **Retrieval quality** — add a grounded task to `benchmarks/tasks/` and its
  expected symbols to `benchmarks/truth/`, then show before/after numbers.
- **Language support** — add a tree-sitter adapter for a new language
  (see [Adding a language](#adding-a-language)).
- **Documentation** — fixes to guides, examples, and error messages are always
  welcome.
- **Performance** — include a reproducible benchmark and the machine/profile
  used. "It feels faster" is not enough.
- **Tests** — regression tests for parser, storage, ranking, and MCP behaviour.

If you are unsure whether a change fits the project, open a discussion issue
first and describe the problem before writing code.

## Development setup

### Requirements

- **Rust stable** (the workspace uses edition 2021; CI tracks stable).
- **Git**.
- A C/C++ toolchain for `rusqlite`/`tantivy`/`fastembed` native dependencies.
- Python 3 only for benchmark and distribution scripts.
- Network access on the first `lx index` (the embedding model is downloaded
  once). Building and testing do not need the model.

macOS Intel (x86_64) is not supported because upstream ONNX Runtime no longer
publishes an x86_64 macOS build. Use Apple Silicon or Linux.

### Build and run

```bash
git clone https://github.com/PizenLabs/lynx
cd lynx

cargo build --release -p pizen-lynx     # target/release/lx
./target/release/lx --version
./target/release/lx doctor
```

For a fast edit/run loop, use the debug binary:

```bash
cargo run -p pizen-lynx -- --help
cargo run -p pizen-lynx -- index .
```

### Quality gates

These are the same gates CI runs. Run them before opening a pull request:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Useful targets:

| Command | What it covers |
|---|---|
| `make fmt` | Format the workspace |
| `make ci` | Format check, check, clippy, tests |
| `make dist-test` | Installer + clean-machine install → index → query → MCP |
| `tests/distribution/release_test.sh` | Release classification and version guards |
| `tests/distribution/cargo_install_test.sh` | Package every crate and install from a local registry |
| `benchmarks/scripts/run_final.sh` | Full reproducible benchmark suite |

Distribution and benchmark scripts can take several minutes and may need a
release build first. They are not required for a documentation-only change.

## Repository layout

```text
crates/
  lynx-protocol/   frozen contracts (no I/O); the stability boundary
  lynx-parser/     source -> SymbolIdentity + intra-file Relation
  lynx-embed/      text -> vector (VectorProvider seam; local FastEmbed)
  lynx-storage/    SQLite graph/vector substrate + Tantivy lexical index
  lynx-core/       engine: discovery, indexing, retrieval, ranking, context
  lynx-mcp/        stdio JSON-RPC transport + session lifecycle
  lynx-cli/        `lx` command-line transport (the product package)

docs/              user guides and architecture records
benchmarks/        deterministic corpora, tasks, truth, and harnesses
tests/             distribution, installer, release, and registry tests
scripts/release/   tag classification and version guards
```

`lynx-core` owns product logic; `lynx-storage` owns persistence; transports
contain neither. Keep it that way.

## What to change, and where

### Retrieval and ranking

`crates/lynx-core/src/engine.rs` and `retrieval.rs`. Ranking changes must
preserve determinism: total orders, no hash-dependent tie-breaks, no
repository-proportional memory. Add a grounded benchmark task and report
P@1/@3, R@5/@10, Hit@5, and MRR before and after.

### Parser and languages

`crates/lynx-parser/src/adapter.rs` owns adapters. Relations are **intra-file**
by construction: an edge is emitted only when both endpoints come from the same
source file. Do not add cross-file linkage without a design discussion.

### Storage

`crates/lynx-storage/`. The SQLite substrate stores identity, graph, and
vectors; Tantivy stores lexical documents. Schema changes must handle existing
indexes (`freshness` reports `incompatible` rather than silently misreading
them) and need lifecycle/corruption tests.

### MCP surface

`crates/lynx-mcp/src/tools.rs` defines the eight tools. The tool surface is
consumer-agnostic; do not add vendor-specific tools. Keep stdout protocol-only
and all diagnostics on stderr.

### CLI surface

`crates/lynx-cli/src/main.rs`. Commands are documented in
[`docs/cli.md`](docs/cli.md); update the reference and `lx --help` together.

### Wire contracts

`crates/lynx-protocol/` is frozen. The serialized form is the contract:
adding a field is a wire change; renaming or removing one is breaking.
Update `crates/lynx-protocol/tests/wire_contract.rs` and the relevant docs in
the same pull request.

## Adding a language

1. Add a `Language` variant if the format is not already represented in
   `crates/lynx-protocol/src/language.rs`.
2. Implement `LanguageAdapter` in `crates/lynx-parser/src/adapter.rs` using the
   matching `tree-sitter-*` grammar. Grammar-backed adapters report
   `CapabilityLevel::L3Structural` and emit intra-file relations.
3. Register the extension in `language_for_path` and the adapter dispatch in
   `adapter_for_path` (`crates/lynx-parser/src/lib.rs`).
4. Add parser tests for symbols, methods, nested declarations, and at least one
   relation. Add a fixture if the grammar has unusual syntax.
5. Update the supported-languages list in the README and
   [`docs/concepts.md`](docs/concepts.md).
6. Run the full quality gates. If indexing behaviour changes, run the
   benchmark suite and include the result.

Grammar-less formats should use `GenericAdapter` and degrade to `L1Parsed`
rather than pretending to be structural.

## Commit and pull-request conventions

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(parser): add Ruby adapter
fix(storage): reopen the lexical index after a corrupt segment
perf(core): stream vectors instead of collecting them
docs(mcp): document the OpenCode configuration
test(core): cover deleted-file incremental updates
```

Keep commits focused and the subject imperative. A pull request should:

- explain the **problem**, not just the patch;
- link the issue it closes (`Closes #123`);
- include tests for behaviour changes;
- update user-facing docs when the CLI, MCP surface, limits, or installation
  changes;
- include benchmark numbers for performance or ranking changes;
- note any wire-contract or index-format compatibility impact.

The [pull request template](.github/PULL_REQUEST_TEMPLATE.md) lists the full
checklist.

## Documentation style

- Prefer short, testable statements over marketing language.
- Use `lx` for the binary, `Lynx` for the product, and `pizen-lynx` for the
  Cargo package.
- Use relative links between repository files; absolute links for crates.io
  and docs.rs pages.
- Keep line width near 80 columns for prose.
- Do not claim performance, quality, or platform support without a
  reproducible source in `benchmarks/` or CI.
- Clearly label historical reports as historical. Current guarantees belong in
  the README, `docs/concepts.md`, and the architecture contract documents.

Markdown links are checked in CI by `scripts/docs/check_links.py`. Run it
locally with:

```bash
python3 scripts/docs/check_links.py
```

## Releases

Releases are maintainer-only and tag-driven:

1. Bump `[workspace.package] version` and the internal
   `[workspace.dependencies]` versions in `Cargo.toml`.
2. Commit the bump.
3. Tag `vMAJOR.MINOR.PATCH` and push the tag.
4. The release workflow classifies the tag, verifies it matches the package
   version, builds native archives, publishes `SHA256SUMS`, and creates the
   GitHub Release.
5. crates.io publication is a separate, manual workflow.

Do not create tags from a fork. See
[`docs/architecture/release-contract.md`](docs/architecture/release-contract.md)
for the normative rules.

## License

By contributing, you agree that your contribution is licensed under the
[MIT License](LICENSE). No CLA is required.
