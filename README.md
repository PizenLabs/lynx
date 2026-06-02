<div align="center">

<img src="https://raw.githubusercontent.com/PizenLabs/onpic/refs/heads/main/lynx/lynx.png" width="28%" alt="Lynx Logo" />

# Lynx

### Symbol-first repository discovery engine for AI-native developer tooling.

**Lynx transforms developer intent into stable repository coordinates — symbols, files, and structural chunks — enabling downstream reasoning systems like Lea to operate on deterministic code primitives instead of fragile text spans.**

[![Crates.io](https://img.shields.io/crates/v/pizen-lynx?style=flat-square&color=orange)](https://crates.io/crates/pizen-lynx)
[![Docs.rs](https://img.shields.io/docsrs/pizen-lynx?style=flat-square&color=blue)](https://docs.rs/pizen-lynx)
[![CI](https://img.shields.io/github/actions/workflow/status/PizenLabs/lynx/ci.yml?branch=main&style=flat-square)](https://github.com/PizenLabs/lynx/actions)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](./LICENSE)
[![Stars](https://img.shields.io/github/stars/PizenLabs/lynx?style=flat-square&color=gold)](https://github.com/PizenLabs/lynx/stargazers)

---

 **Lynx discovers. Lea reasons.**

[Features](#features) •
[Architecture](#architecture-high-level) •
[Installation](#installation) •
[Usage](#usage) •
[MCP Server](#mcp-server-json-lines) •
[Repository Layout](#repository-layout) •
[Contributing](#contributing)

</div>


## Features

- **Symbol-first discovery** with stable, deterministic identifiers.
- **Hybrid retrieval**: BM25 + semantic embeddings with Reciprocal Rank Fusion (RRF).
- **Local-first, CPU-first** design with no cloud or GPU dependency.
- **Tree-sitter parsing** for structured symbol extraction and chunking.
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

## Installation

Install the CLI from crates.io:

```bash
cargo install pizen-lynx
```

The binary name is **`lx`**.

## Usage

Index a repository:

```bash
lx index /path/to/repo
```

Search the index:

```bash
lx search "authentication flow"
```

Resolve a symbol:

```bash
lx resolve Login
```

Find related implementations:

```bash
lx related internal/auth/service.go:42
```

To change the storage location (default: `.lynx`):

```bash
lx --storage-path /tmp/lynx index .
```

### Development

```bash
cargo run -p pizen-lynx -- search "jwt validation"
```

## MCP Server (JSON Lines)

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
  lynx-cli/       # CLI tool (crate: pizen-lynx)
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

## Contributing

Issues and pull requests are welcome. Please run `make ci` before submitting.

## License

MIT
