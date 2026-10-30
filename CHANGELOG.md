# Changelog

All notable changes to Lynx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While Lynx is pre-1.0, a minor version may contain breaking changes; the
serialized wire form is versioned by `pizen-lynx-protocol`.

## [Unreleased]

### Added

- Open-source community files: contributing guide, code of conduct, security
  policy, support guide, issue forms, and pull-request template.
- A documentation hub and new user guides: getting started, concepts,
  troubleshooting, and FAQ.
- `scripts/docs/check_links.py`, a relative-link checker run in CI.
- A warm-query MCP probe and a parser single-pass benchmark.
- A measured [performance and productization audit](docs/audit/lynx-performance-and-productization-report.md).

### Changed

- Each crate now has a crate-specific README instead of a copy of the product
  README. The internal crates clearly state that they are not a public API.
- The embedding model is constructed lazily on the first embedding call.
  Structural commands, lexical search, no-change indexing, and MCP startup on an
  existing index no longer load it.
- Indexing reads the workspace once, parses each file once, uses prepared
  statements and linear-time change selection, and resolves git provenance
  without a subprocess on the common path.
- Lexical search expands camelCase and snake_case identifiers.
- Embedding batches are bounded, preventing dense-file memory blow-ups.
- **Index format:** the graph schema is now 4 and the lexical schema 2. Existing
  indexes report `incompatible`; run `lx index` (or `lx index --force`) once to
  rebuild. This does not change the serialized protocol.

### Fixed

- Documentation links and version examples that pointed at stale paths or
  versions.
- The macOS Intel note no longer suggests a source build, which is not possible
  without an upstream ONNX Runtime x86_64 macOS build.
- No-change and deletion-only index passes no longer pay the model-load cost.

## [0.2.1] - 2026-10-17

### Changed

- Removed macOS Intel (x86_64) from the release target matrix. ONNX Runtime no
  longer publishes an x86_64 macOS build, so the target cannot be built.
- Updated dependencies, including `fastembed` and `rusqlite`.
- Clarified the release checkout requirements for publishing a GitHub Release.

## [0.2.0] - 2026-10-16

### Added

- Persistent, incremental index at `<workspace>/.lynx`, backed by SQLite
  (identity, relations, vectors) and Tantivy (lexical).
- Frozen protocol contracts in `pizen-lynx-protocol`.
- Tree-sitter parsers for Rust, Go, TypeScript, JavaScript, and Python, with
  symbol extraction and intra-file relations. Markdown, YAML, JSON, TOML, and
  generic text are indexed as bounded file/module documents.
- `lx` CLI: `index`, `search`, `resolve`, `relations`, `context`, `status`,
  `doctor`, and `mcp`.
- Stdio MCP server with eight consumer-agnostic tools: `search`, `resolve`,
  `resolve_all`, `inspect`, `relations`, `similar`, `context`, and
  `index_status`.
- `lx mcp --check` protocol smoke test and `lx doctor` diagnostics.
- One-command installer with SHA-256 verification, atomic replacement, and no
  `sudo` requirement.
- Distribution tests: installer behaviour, clean-machine install → index →
  query → MCP, release classification, and local-registry `cargo install`.
- crates.io publication of `pizen-lynx` and its internal dependency crates.
- Bounded query lifecycle: candidate caps, wall-clock deadlines, and
  cooperative cancellation.
- Reproducible benchmark suite covering retrieval quality, scaling, incremental
  indexing, persistence lifecycle, MCP stress, and a ripgrep baseline.

### Changed

- Queries hydrate the index and never re-index; only `lx index` writes it.
- Removed repository-proportional in-memory state; semantic scans stream
  vectors from storage.
- Ranking uses symmetric reciprocal rank fusion with deterministic,
  hash-independent tie-breaks.

## Earlier releases

The project was developed in public before the 0.2 line. Earlier tags are kept
for history; their binaries predate the persistent-index architecture.

| Version | Date |
|---|---|
| `0.1.3` | 2026-06-22 |
| `0.1.2` | 2026-06-21 |
| `0.1.1` | 2026-06-05 |
| `0.0.3` | 2026-06-05 |
| `0.0.2` | 2026-06-05 |
| `0.0.1` | 2026-06-03 |

[Unreleased]: https://github.com/PizenLabs/lynx/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/PizenLabs/lynx/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PizenLabs/lynx/compare/v0.1.3...v0.2.0
