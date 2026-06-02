# Lynx

Lynx is the discovery layer of the PizenLabs ecosystem. It converts human intent into precise repository coordinates (symbols, files, chunks) so downstream tools like Lea can reason about structure and impact.

**Lynx discovers. Lea reasons.**

## Features

- **Symbol-first discovery** with stable, deterministic identifiers.
- **Hybrid retrieval**: BM25 + semantic embeddings with Reciprocal Rank Fusion (RRF).
- **Local-first, CPU-first** design with no cloud or GPU dependency.
- **Tree-sitter chunking** for structured code segmentation.
- **Minimal MCP interface**: `search`, `resolve_symbol`, `find_related`.

## Architecture (High Level)

```
Human Request
     │
     ▼
    Lynx  (Discovery)
     │
     ▼
  Symbol IDs
     │
     ▼
    Lea  (Reasoning)
```

## Tech Stack

- **Rust** (core)
- **Tantivy** (BM25 indexing)
- **Tree-sitter** (parsing + chunking)
- **FastEmbed** (local embeddings)
- **Serde** (serialization)

## Getting Started

### Prerequisites

- Rust toolchain (stable)

### Build

```bash
cargo build
```

### CLI Usage (binary: `lx`)

Index a repository:

```bash
cargo run -p lynx-cli -- index /path/to/repo
```

Search the index:

```bash
cargo run -p lynx-cli -- search "authentication flow"
```

Resolve a symbol:

```bash
cargo run -p lynx-cli -- resolve Login
```

Find related implementations:

```bash
cargo run -p lynx-cli -- related internal/auth/service.go:42
```

### MCP Server (JSON Lines)

Run:

```bash
cargo run -p lynx-mcp -- .lynx
```

Send JSON per line on stdin:

```json
{"method":"search","params":{"query":"jwt validation"}}
```

```json
{"method":"resolve_symbol","params":{"name":"Login"}}
```

```json
{"method":"find_related","params":{"file":"internal/auth/service.go","line":42}}
```

## Repository Layout

```
crates/
  lynx-cli/       # CLI tool
  lynx-core/      # Retrieval pipeline + ranking
  lynx-embed/     # Embeddings (FastEmbed)
  lynx-mcp/       # MCP server
  lynx-parser/    # Tree-sitter symbol extraction
  lynx-protocol/  # Shared structs
  lynx-storage/   # Tantivy index + embedding cache
```

## Project Principles

- Lynx focuses strictly on discovery; reasoning and impact analysis are delegated to Lea.
- Results prioritize **symbol IDs** over raw snippets whenever possible.

## License

MIT
