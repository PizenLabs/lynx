# Lynx CLI reference (`lx`)

`lx` is the command-line transport over the Lynx repository-intelligence
surface. Retrieval subcommands **hydrate** the persistent index at
`<repo>/.lynx`; only `lx index` writes it.

## Install

All channels install the same `lx` binary:

```bash
curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh   # fastest
cargo install pizen-lynx                                                          # Rust-native
git clone https://github.com/PizenLabs/lynx && cargo build --release -p pizen-lynx # source
```

Verify with `lx --version` and `lx doctor`. The installer is HTTPS-only,
checksum-verified, atomic, and needs no `sudo`; it never edits shell startup
files. See the [README](../README.md#install) for details.

```
lx <command> [options]
```

Run `lx --help` or `lx <command> --help` for the same information inline.

## Version and diagnosis

### `lx --version`

Prints `lx X.Y.Z` and exits. Does not load the embedding model. Safe for scripts.

### `lx doctor`

A fast, model-free diagnostic of the installation and workspace: binary,
version, platform, install path, workspace, state writability, repository
detection, index presence/compatibility/freshness, and the MCP protocol
contract. It never indexes and never loads the model (≈37 ms).

```text
Lynx Doctor

Binary       OK
Version      0.2.0
Platform     macOS arm64
Install      /Users/you/.local/bin/lx
Workspace    /Users/you/project
State path   /Users/you/project/.lynx
Permissions  OK
Repository   detected (git)
Index        fresh (128 symbols)
Capability   L3Structural
MCP          OK

Status       READY
```

`Status READY` requires a fresh index and a passing MCP check; otherwise it
reports `NOT READY` with targeted next steps. Exit status is non-zero only if
the diagnosis itself cannot run.

## Repository workflow

### `lx index [PATH] [--include-tests] [--force]`

Builds or incrementally updates the index. `PATH` defaults to the current
directory. Only added, modified, and deleted files are re-parsed and re-embedded.
`--force` discards and rebuilds from scratch (the recovery path). `--include-tests`
includes test, mock, and generated files.

### `lx status [--no-check]`

Counts, provenance, capability, and freshness (`fresh`, `stale`, `missing`,
`incompatible`, `corrupt`, `unknown`). `--no-check` skips the workspace
re-scan and prints counts only.

### `lx search <QUERY> [--limit N] [--mode lexical|semantic|structural|hybrid] [--explain] [--timeout-ms N] [--candidate-limit N]`

Hybrid (default) BM25 + embedding search fused by reciprocal rank fusion.
`--explain` prints the query report and per-result ranking contributions.

### `lx resolve <NAME>`

Exact symbol coordinates by fully-qualified name or trailing name. Lists
alternatives when a name is ambiguous.

### `lx relations <SYMBOL>`

Structural edges incident to a symbol (intra-file).

### `lx context <QUERY> [--token-budget N]`

A token-bounded `ContextPackage` as JSON. Default budget: 2048 tokens.

### `lx mcp [PATH] [--include-tests] [--check]`

Launches the canonical stdio MCP server (see [`mcp.md`](mcp.md)). `PATH`
defaults to the enclosing git toplevel of the CWD; pass it explicitly when the
launch directory is ambiguous. `--check` runs a fast, non-indexing MCP protocol
smoke test and exits.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | success (or a completed `doctor` diagnosis) |
| non-zero | a clear, actionable error was reported on stderr |

## Environment

| Variable | Effect |
|---|---|
| `LYNX_INCLUDE_TESTS=1` | include test/mock/generated files in a freshly built index (`lx index`, `lx mcp`) |

Lynx reads no other environment variables. There is no global configuration file
and no product-specific repository marker.
