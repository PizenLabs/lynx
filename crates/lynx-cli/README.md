<div align="center">

<img src="https://raw.githubusercontent.com/PizenLabs/onpic/refs/heads/main/lynx/lynx.png" width="28%" alt="Lynx Logo" />

# Lynx

### A deterministic repository intelligence engine for AI-native developer tooling.

**Lynx discovers. Consumers decide.**

[![CI](https://github.com/PizenLabs/lynx/actions/workflows/ci.yml/badge.svg)](https://github.com/PizenLabs/lynx/actions)
[![crates.io](https://img.shields.io/crates/v/pizen-lynx.svg?style=flat-square)](https://crates.io/crates/pizen-lynx)
[![docs.rs](https://img.shields.io/docsrs/pizen-lynx?style=flat-square)](https://docs.rs/pizen-lynx)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](https://github.com/PizenLabs/lynx/blob/main/LICENSE)

</div>

---

Lynx turns a repository into stable symbols, exact coordinates, structural
relations, and embeddings, then serves them to tools and agents through the `lx`
CLI and a stdio MCP server.

**Lynx discovers. Consumers decide.**

## Install

```bash
cargo install pizen-lynx
lx --version
lx doctor
```

The fastest channel is the installer, which needs no Rust toolchain:

```bash
curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh
```

## Quick start

```bash
cd /path/to/your/repository

lx index                                # build <repo>/.lynx
lx status                               # counts, provenance, freshness
lx search "authentication middleware"   # ranked symbols
lx resolve ValidateToken                # exact coordinates
lx context "token refresh" --token-budget 2048
```

Connect any MCP client:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

## Documentation

- [Documentation hub](https://github.com/PizenLabs/lynx/blob/main/docs/README.md)
- [Getting started](https://github.com/PizenLabs/lynx/blob/main/docs/getting-started.md)
- [CLI reference](https://github.com/PizenLabs/lynx/blob/main/docs/cli.md)
- [MCP reference](https://github.com/PizenLabs/lynx/blob/main/docs/mcp.md)
- [Architecture](https://github.com/PizenLabs/lynx/blob/main/docs/architecture/README.md)

## Supported surface

This package installs the only supported product entry point, the `lx` binary.
The internal `pizen-lynx-*` crates are published only because Cargo requires a
registry-resolvable dependency graph; they are not a public API. Lynx is
pre-1.0; a minor release may contain breaking changes.

## License

MIT — see [LICENSE](LICENSE).
