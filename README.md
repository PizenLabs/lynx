<div align="center">

<img src="https://raw.githubusercontent.com/PizenLabs/onpic/refs/heads/main/lynx/lynx.png" width="28%" alt="Lynx Logo" />

# Lynx

### A deterministic repository intelligence engine for AI-native developer tooling.

**Lynx discovers. Consumers decide.**

[![CI](https://github.com/PizenLabs/lynx/actions/workflows/ci.yml/badge.svg)](https://github.com/PizenLabs/lynx/actions)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](./LICENSE)

[What is Lynx](#what-is-lynx) •
[Why](#why-lynx-exists) •
[How agents use it](#how-an-agent-uses-lynx) •
[Install](#installation) •
[CLI](#cli) •
[MCP](#mcp-server) •
[Indexes](#indexes-and-lifecycle) •
[Limits](#supported-scope-and-limits) •
[Architecture](#architecture)

</div>

---

## What is Lynx?

Lynx answers a single class of question extremely well:

> **Where, in this repository, does X live — and what is it connected to?**

It converts developer intent ("where is token refresh handled?", "who calls
`ValidateToken`?", `auth::AuthService::Login`) into **stable repository
coordinates** — a fully-qualified symbol name, its file, and its exact line
range — plus the structural relations around it and an optional token-bounded
context package.

Lynx is **not** an agent, a planner, or an editor. It does not reason about what
you should do, modify your code, or decide anything. It **discovers**, and it
returns evidence; the consumer decides.

Lynx is a standalone product. It has no dependency on any particular agent, IDE,
or MCP client.

## Why Lynx exists

AI agents and developer tools need reliable, fast, structured access to
repository knowledge. Without a repository intelligence layer they repeatedly
scan, grep, embed, and guess their way through the codebase — every query,
every session.

Lynx gives them one persistent, deterministic layer instead:

- **Index once, query cheaply.** `lx index` writes a persistent `.lynx` index;
  search/resolve/relations/context *hydrate* it and never re-index. Queries are
  `O(query)`, not `O(repository)`.
- **Coordinates, not text spans.** Results are symbols with stable identities,
  so they survive across runs and processes.
- **Deterministic.** Bare symbol lookups bypass ranking entirely.
- **Bounded.** Every query has a candidate cap, a deadline, and a cancellation
  token. Memory does not scale with repository size.
- **Consumer-agnostic.** A small CLI and a standard stdio MCP server. No SDK.

## Why not just grep or a vector search?

They answer different questions:

| Task | Best tool |
|---|---|
| Find a literal string / regex in files | **ripgrep** |
| "I want the *symbol* `ValidateToken`, wherever it is" | **Lynx** (`resolve`) |
| "What calls this function?" | **Lynx** (`relations`) |
| "Where is authentication implemented?" (paraphrase) | **Lynx** (`search`) |
| "Give me the smallest context to change this behavior" | **Lynx** (`context`) |
| Semantic similarity over an arbitrary corpus | a vector database |

Lynx does not try to beat ripgrep at raw text search. It wins where repository
*structure* matters: symbol resolution, structural navigation, relations,
conceptual discovery, and persistent repository knowledge. On the same grounded
tasks, ripgrep is faster but ranks worse whenever relevance or paraphrase
matters — see `benchmarks/scripts/rg_baseline.py`.

## How an agent uses Lynx

An agent connects over MCP and asks repository questions in natural language or
by symbol name. A typical navigation workflow is one or two calls:

1. `search("jwt authentication")` → ranked symbols with coordinates.
2. `context("jwt authentication", token_budget=2048)` → a bounded package.
3. `relations(symbol_hash)` → the call graph around the chosen symbol.

Each result is a grounded coordinate the agent can act on. Measured on the
reference corpus, these workflows reach the right symbol in 1–2 calls at a few
milliseconds each (see the final product report).

---

## Installation

```bash
cargo install pizen-lynx      # installs the `lx` binary
```

Build from source:

```bash
cargo build --release -p pizen-lynx
```

Requires a Rust toolchain. Embeddings run locally on CPU (no GPU, no cloud).

---

## CLI

### Index a repository

```bash
lx index /path/to/repo             # build or incrementally update <repo>/.lynx
lx index /path/to/repo --force     # discard and rebuild from scratch
lx index /path/to/repo --include-tests
```

`lx index` only re-parses and re-embeds files that were added, modified, or
deleted since the last pass. Test, mock, and generated files are skipped by
default.

### Search (conceptual / lexical)

```bash
lx search "jwt validation token"
lx search "jwt validation token" --mode semantic --limit 5 --explain
```

### Resolve a symbol (deterministic)

```bash
lx resolve Login                  # exact identity coordinates; lists alternatives
```

### Relations

```bash
lx relations Validate             # call/containment edges incident to a symbol
```

### Context package

```bash
lx context "user validation handler" --token-budget 2048   # JSON ContextPackage
```

### Status and freshness

```bash
lx status                         # counts, provenance, and freshness
lx status --no-check              # skip the workspace re-scan
```

### MCP server

```bash
lx mcp                            # stdio MCP server for the current workspace
```

Retrieval commands hydrate `<repo>/.lynx`; they never re-index the repository.
Only `lx index` writes it.

---

## MCP server

Lynx ships a minimal stdio **Model Context Protocol** server. Any MCP client can
use it without Lynx-specific knowledge; internal storage (SQLite, Tantivy, ONNX)
is never exposed.

| Tool | Arguments | Returns |
|---|---|---|
| `search` | `query`, `mode?`, `limit?`, `timeout_ms?` | Ranked `Evidence` (hybrid RRF) |
| `resolve` | `name` | The deterministic best symbol, or null |
| `resolve_all` | `name` | Every symbol matching a name (ambiguity) |
| `inspect` | `symbol_hash` | Full evidence for one symbol |
| `relations` | `symbol_hash`, `kind?` | Structural edges incident to a symbol |
| `similar` | `symbol_hash`, `timeout_ms?` | Embedding-similarity neighbors |
| `context` | `query`, `token_budget?`, `timeout_ms?` | Budget-bounded `ContextPackage` |
| `index_status` | `path?`, `check_freshness?` | Counts, provenance, freshness |

Example frame:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{"query":"authentication flow"}}}
```

The server hydrates the workspace's `.lynx` index at startup (building it once
if absent), speaks newline-delimited JSON-RPC 2.0, and contains every malformed
frame, unknown tool, and timeout. Diagnostics go to stderr; stdout carries only
protocol frames.

---

## Indexes and lifecycle

```text
lx index   ->  <repo>/.lynx/   graph.sqlite3 (identities, relations, vectors)
                               lexical/      (BM25 snippet index)
           ->  exit
lx search / resolve / relations / context / status   (hydrate, query)
```

- **Freshness** is explicit: `fresh`, `stale`, `missing`, `incompatible`,
  `corrupt`, or `unknown`. Lynx never silently serves a stale index as current —
  `lx status` and MCP `index_status` tell you, and `lx index` refreshes it.
- **Incremental**: adding, editing, deleting, renaming, or moving files updates
  only the affected state; no stale symbol, vector, or document survives.
- **Recovery**: an incompatible schema resets to a clean substrate; a corrupt
  artifact is detected and reported; `lx index --force` rebuilds.

---

## Supported scope and limits

**Languages.** Rust, Go, TypeScript, JavaScript, and Python are parsed to
symbols (functions, methods, types, traits, interfaces, enums, modules) and
intra-file relations. Markdown, YAML, JSON, TOML, and unknown text files are
indexed as bounded file/module documents.

**Explicit limitations** (not hidden):

- **Semantic top-1** is representation-limited. Paraphrase *recall* is strong
  (the right symbol is almost always in the top results), but the top result is
  not always the right one. Lexical and structural lookups are precise.
- **Relations are intra-file.** Cross-file calls are not linked.
- **Context ordering** inherits retrieval ordering: the right symbol is usually
  in the package, not always first.
- **Very large repositories** are supported up to the measured envelope;
  semantic retrieval is an exact, `O(symbols)` scan (no approximate index), so
  beyond the envelope cost grows linearly.
- **Resource requirements** are dominated by the embedding model (~250 MB
  resident). Query memory itself is small and bounded; structural commands
  (`resolve`, `relations`, lexical-only search, `status`) load no model at all.

Honest, measured numbers for all of the above are in
[`docs/architecture/final-product-report.md`](docs/architecture/final-product-report.md)
and [`docs/architecture/production-readiness.md`](docs/architecture/production-readiness.md).

---

## Architecture

```text
crates/
  lynx-protocol/   frozen contracts every consumer depends on (no I/O)
  lynx-parser/     source -> SymbolIdentity + intra-file Relation
  lynx-embed/      text -> vector (VectorProvider seam; local FastEmbed)
  lynx-storage/    DualStorage: SQLite (identity/graph/vectors) + Tantivy (lexical)
  lynx-core/       Engine: discovery, indexing, retrieval, ranking, context
  lynx-mcp/        stdio JSON-RPC transport + session lifecycle
  lynx-cli/        `lx` command-line transport
```

`lynx-core` owns the product logic; `lynx-storage` owns persistence; the
transports contain none. Nothing repository-proportional is retained in memory:
queries stream from storage under a bounded candidate budget. See
[`docs/architecture/final-product-audit.md`](docs/architecture/final-product-audit.md).

---

## Reproducible benchmarks

```bash
benchmarks/scripts/run_final.sh        # writes benchmarks/reports/final_<ts>.txt
```

Runs the grounded retrieval suite, repository scaling, incremental indexing,
persistence lifecycle, MCP stress, agent workflows, and a ripgrep baseline.

---

## Contributing

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## License

MIT
