<div align="center">

<img src="https://raw.githubusercontent.com/PizenLabs/onpic/refs/heads/main/lynx/lynx.png" width="28%" alt="Lynx Logo" />

# Lynx

### A deterministic repository intelligence engine for AI-native developer tooling.

**Lynx discovers. Consumers decide.**

[![CI](https://github.com/PizenLabs/lynx/actions/workflows/ci.yml/badge.svg)](https://github.com/PizenLabs/lynx/actions)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](./LICENSE)

[Install](#install) •
[First repository](#first-repository) •
[MCP](#connect-an-agent-mcp) •
[CLI](#cli) •
[Platforms](#supported-platforms) •
[Releases](#releases) •
[Limits](#resource-and-support-envelope) •
[Architecture](#architecture)

</div>

---

## What is Lynx?

Lynx answers a single class of question extremely well:

> **Where, in this repository, does X live — and what is it connected to?**

It turns developer intent ("where is token refresh handled?", "who calls
`ValidateToken`?", `auth::AuthService::Login`) into **stable repository
coordinates** — a fully-qualified symbol name, its file, and its exact line
range — plus the structural relations around it and an optional token-bounded
context package.

Lynx is **not** an agent, a planner, or an editor. It does not reason about what
you should do, modify your code, or decide anything. It **discovers**, and it
returns evidence; the consumer decides.

Lynx is standalone. It has no dependency on any particular agent, IDE, or MCP
client.

## Why Lynx exists

AI agents and developer tools need reliable, fast, structured access to
repository knowledge. Without a repository intelligence layer they repeatedly
scan, grep, embed, and guess their way through the codebase — every query, every
session.

Lynx gives them one persistent, deterministic layer instead:

- **Index once, query cheaply.** `lx index` writes a persistent `.lynx` index;
  search/resolve/relations/context *hydrate* it and never re-index. Queries are
  `O(query)`, not `O(repository)`.
- **Coordinates, not text spans.** Results are symbols with stable identities, so
  they survive across runs and processes.
- **Deterministic.** Bare symbol lookups bypass ranking entirely.
- **Bounded.** Every query has a candidate cap, a deadline, and a cancellation
  token. Memory does not scale with repository size.
- **Consumer-agnostic.** A small CLI and a standard stdio MCP server. No SDK.

---

## Install

Three supported channels — all install the same `lx` binary and the same
`lx mcp` server.

**1. Installer (fastest; no Rust/Node/Python/Docker runtime required):**

```bash
curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh
```

**2. Rust-native (`cargo install`):**

```bash
cargo install pizen-lynx          # installs the `lx` binary
```

**3. Source (contributors):**

```bash
git clone https://github.com/PizenLabs/lynx
cd lynx && cargo build --release -p pizen-lynx   # target/release/lx
```

Then verify whichever channel you used:

```bash
lx --version
lx doctor
```

The installer detects your OS and architecture, downloads the matching prebuilt
archive, **verifies its SHA-256 checksum**, installs the `lx` binary atomically
into `$HOME/.local/bin`, and prints PATH guidance if needed. It needs no `sudo`.

Inspect or control it without executing:

```bash
sh install.sh --help                 # usage
sh install.sh --download-only=/tmp   # download + verify only, install nothing
LYNX_VERSION=v0.2.0 sh install.sh    # pin a release
sh install.sh --dir "$HOME/bin"      # choose the install directory
sh install.sh --system               # explicit /usr/local/bin install (may need sudo)
```

Safety properties: HTTPS-only, checksum verified before unpacking, atomic
replacement (a failed install leaves your existing `lx` untouched), no shell
startup files modified, no telemetry, no background service. The installer is a
single auditable file: [`install.sh`](./install.sh).

> **crates.io note.** `pizen-lynx` is the product package. Its internal
> implementation crates (`pizen-lynx-core`, `-storage`, `-parser`, `-embed`,
> `-protocol`, `-mcp`) are published only because Cargo requires them for
> `cargo install`; they are **not** a public API and carry no stability
> guarantee. See
> [`docs/architecture/distribution-design.md`](docs/architecture/distribution-design.md#13-public-surface-and-stability).

> **First index note.** The embedding model is downloaded once into
> `$HOME/.cache/lynx/fastembed` on the first `lx index`. Nothing else is
> downloaded after install.

---

## First repository

From any repository (git not required):

```bash
cd /path/to/your/repository

lx doctor                          # health check: state, index, MCP
lx index                           # build or incrementally update <repo>/.lynx
lx status                          # counts, provenance, freshness

lx search "authentication middleware"         # ranked symbols
lx resolve ValidateToken                       # exact coordinates
lx context "token refresh" --token-budget 2048 # bounded context package
```

`lx index` only re-parses and re-embeds files that changed since the last pass.
Retrieval commands hydrate `<repo>/.lynx`; only `lx index` writes it.

An agent needs only one more step: see below.

---

## Connect an agent (MCP)

Lynx ships one canonical stdio MCP server, usable by any MCP client:

```bash
lx mcp                 # serve the current repository over stdio
lx mcp /path/to/repo   # or pin the workspace explicitly
lx mcp --check         # fast protocol smoke test (no indexing), then exit
```

Point your agent at the installed binary with the standard stdio shape:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

That is the whole integration: the consumer spawns `lx`, discovers the tools, and
queries the repository. Per-consumer file locations and snippets (Claude Desktop,
Claude Code, Cursor, VS Code/Copilot, OpenCode, Cline/Windsurf, Zed, Gemini CLI)
are in [`docs/mcp.md`](docs/mcp.md).

| Tool | Returns |
|---|---|
| `search` | Ranked matches (BM25 + embeddings, RRF-fused) |
| `resolve` / `resolve_all` | Deterministic symbol coordinates / ambiguity |
| `inspect` | Full evidence for one symbol |
| `relations` | Structural edges incident to a symbol |
| `similar` | Embedding-similarity neighbors |
| `context` | Budget-bounded `ContextPackage` |
| `index_status` | Counts, provenance, freshness |

Lynx never edits your agent configuration and never modifies unrelated files.

---

## CLI

| Command | Purpose |
|---|---|
| `lx index [PATH] [--include-tests] [--force]` | Build/refresh the persistent index |
| `lx status [--no-check]` | Counts, provenance, freshness |
| `lx search <QUERY> [--mode …] [--limit N] [--explain]` | Hybrid search |
| `lx resolve <NAME>` | Exact symbol coordinates |
| `lx relations <SYMBOL>` | Structural edges |
| `lx context <QUERY> [--token-budget N]` | Bounded context package |
| `lx doctor` | Installation/index/MCP health check |
| `lx mcp [PATH] [--check]` | stdio MCP server / smoke test |

Full reference: [`docs/cli.md`](docs/cli.md).

---

## Supported platforms

Prebuilt binaries are built natively per target and published with a
`SHA256SUMS` manifest:

| Platform | Architecture | Artifact | Status |
|---|---|---|---|
| macOS | arm64 (Apple Silicon) | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | Validated |
| Linux | x86_64 | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | CI-native |
| Linux | arm64 | `lynx-vX.Y.Z-linux-arm64.tar.gz` | CI-native |
| macOS | x86_64 (Intel) | — | **Unsupported** |
| Windows | — | — | **Unsupported** |

"CI-native" means the target is built and validated on a native runner by the
release workflow. macOS Intel (x86_64) is unsupported because the embedding
engine is backed by ONNX Runtime, which no longer publishes an x86_64 macOS
build; use an Apple Silicon Mac or build from source. See
[`docs/architecture/release-readiness.md`](docs/architecture/release-readiness.md)
for the exact status of each target.

---

## Releases

Releases are triggered by **Git tags**, and the tag's shape decides whether
production binaries are published (the rule lives in
[`scripts/release/classify_tag.sh`](scripts/release/classify_tag.sh)):

| Tag | Meaning |
|---|---|
| `v1.0.1` (stable `vMAJOR.MINOR.PATCH`) | **Stable release** — builds production binaries, checksums, and a GitHub Release |
| `v1.0.1-rc.1`, `v1.0.1-alpha.1`, `v1.0.1-beta.1` | **Prerelease** — validation only; **does not** update the stable release surface |
| `v1`, `v1.0`, `v1.0.0foo`, `release-v1.0.0` | **Invalid** — the pipeline fails closed |

A stable release guarantees `git tag == Cargo package version == lx --version`.
The installer consumes only stable releases: `latest` resolves to the newest
non-prerelease release, and an explicitly requested prerelease tag is refused
unless `LYNX_ALLOW_PRERELEASE=1` is set for testing.

**crates.io publication is separate and maintainer-controlled.** GitHub Releases
do not trigger a crates.io publish. `cargo install pizen-lynx` installs the one
product binary `lx`; the internal crates are published only because Cargo
requires them and carry no stability guarantee.

The full contract — state machine, target policy, installer rules, publication
policy — is [`docs/architecture/release-contract.md`](docs/architecture/release-contract.md).

---

## Resource and support envelope

**Languages.** Rust, Go, TypeScript, JavaScript, and Python are parsed to symbols
(functions, methods, types, traits, interfaces, enums, modules) and intra-file
relations. Markdown, YAML, JSON, TOML, and unknown text files are indexed as
bounded file/module documents.

**Explicit limitations** (not hidden):

- **Semantic top-1** is representation-limited. Paraphrase *recall* is strong
  (the right symbol is almost always in the top results), but the top result is
  not always the right one. Lexical and structural lookups are precise.
- **Relations are intra-file.** Cross-file calls are not linked.
- **Context ordering** inherits retrieval ordering: the right symbol is usually
  in the package, not always first.
- **Very large repositories** are supported up to the measured envelope;
  semantic retrieval is an exact `O(symbols)` scan (no approximate index).
- **Resource requirements** are dominated by the embedding model (~250 MB
  resident). Structural commands (`resolve`, `relations`, lexical-only search,
  `status`, `doctor`, `--version`) load no model at all.

Honest, measured numbers are in
[`docs/architecture/final-product-report.md`](docs/architecture/final-product-report.md)
and [`docs/architecture/production-readiness.md`](docs/architecture/production-readiness.md).

---

## Architecture

The public product is one binary (`lx`) with two surfaces (`lx` and `lx mcp`).
The implementation is a modular Cargo workspace:

```text
crates/
  lynx-protocol/   frozen contracts every consumer depends on (no I/O)
  lynx-parser/     source -> SymbolIdentity + intra-file Relation
  lynx-embed/      text -> vector (VectorProvider seam; local FastEmbed)
  lynx-storage/    DualStorage: SQLite (identity/graph/vectors) + Tantivy (lexical)
  lynx-core/       Engine: discovery, indexing, retrieval, ranking, context
  lynx-mcp/        stdio JSON-RPC transport + session lifecycle
  lynx-cli/        `lx` command-line transport (the product package)
```

`lynx-core` owns the product logic; `lynx-storage` owns persistence; the
transports contain none. Nothing repository-proportional is retained in memory:
queries stream from storage under a bounded candidate budget. See
[`docs/architecture/final-product-audit.md`](docs/architecture/final-product-audit.md).

**Workspace boundaries are implementation boundaries, not public APIs.** Only
`pizen-lynx` (the product) is a supported, versioned package. The other crates
are published to crates.io only because Cargo requires registry-resolvable
dependencies; they are internal and may change at any time. See
[`docs/architecture/distribution-design.md`](docs/architecture/distribution-design.md)
and [`docs/architecture/distribution-audit.md`](docs/architecture/distribution-audit.md).

---

## Reproducible benchmarks

```bash
benchmarks/scripts/run_final.sh        # writes benchmarks/reports/final_<ts>.txt
```

Runs the grounded retrieval suite, repository scaling, incremental indexing,
persistence lifecycle, MCP stress, agent workflows, and a ripgrep baseline.

---

## Development

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Distribution tests (installer + clean-machine install → index → query → MCP):

```bash
make dist-test
# or
tests/distribution/run.sh
```

Release classification and version guards (the release workflow's own logic):

```bash
tests/distribution/release_test.sh
```

Registry / crates.io packaging test (package every crate, install `pizen-lynx`
from a locally assembled registry, run `lx`):

```bash
tests/distribution/cargo_install_test.sh
```

Build a release binary directly:

```bash
cargo build --release -p pizen-lynx     # target/release/lx
```

## License

MIT
