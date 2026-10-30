# Getting started

This guide takes you from a fresh machine to a repository that an agent can
query. It assumes a supported platform: macOS arm64 or Linux x86_64/arm64.

## 1. Install

Pick one channel. All three install the same `lx` binary and the same `lx mcp`
server.

### Installer (no Rust required)

```bash
curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh
```

The installer:

- detects the OS and architecture;
- downloads the matching archive from the latest **stable** GitHub Release;
- verifies its SHA-256 checksum before unpacking;
- installs `lx` atomically into `$HOME/.local/bin`;
- never uses `sudo` and never edits shell startup files.

> The installer and crates.io are separate channels. The installer follows the
> newest stable **GitHub Release**; `cargo install` follows **crates.io**. If
> the release you need is not on GitHub yet, use `cargo install pizen-lynx` or
> build from source.

If `$HOME/.local/bin` is not on your `PATH`, the installer prints the line to
add. To choose a directory, pin a version, or inspect the script without running
it:

```bash
sh install.sh --help
sh install.sh --dir "$HOME/bin"
LYNX_VERSION=v0.2.1 sh install.sh
sh install.sh --download-only=/tmp
```

### Cargo

```bash
cargo install pizen-lynx
```

This compiles the product package and installs the `lx` binary. The internal
`pizen-lynx-*` crates are implementation details; do not depend on them
directly.

### From source

```bash
git clone https://github.com/PizenLabs/lynx
cd lynx
cargo build --release -p pizen-lynx
./target/release/lx --version
```

Add `target/release` to `PATH`, or copy the binary somewhere on it.

### Verify

```bash
lx --version
lx doctor
```

`lx doctor` checks the binary, platform, install path, workspace, state
directory permissions, repository detection, index presence/freshness, and the
MCP protocol contract. A fresh install in a repository with no index reports
`Status NOT READY` with the next step (`lx index`); that is expected.

## 2. Index a repository

From any repository — Git is optional:

```bash
cd /path/to/your/repository
lx index
```

The first run downloads the embedding model once into
`$HOME/.cache/lynx/fastembed` (roughly 250 MB resident when loaded) and builds
the persistent index at `<repository>/.lynx`:

```text
.lynx/
├── graph.sqlite3    symbol identity, relations, vectors, provenance
└── lexical/         BM25 documents
```

Subsequent runs are incremental: only added, modified, and deleted files are
re-parsed and re-embedded.

```bash
lx index                 # incremental update
lx index --include-tests # also index test, mock, and generated paths
lx index --force         # discard and rebuild from scratch
lx status                # counts, provenance, freshness
```

Useful details:

- Files larger than 8 MiB are skipped.
- `.git`, `.lynx`, `.fastembed_cache`, `node_modules`, `vendor`, `target`,
  `build`, and `dist` are always excluded.
- Test/mock/generated directories are excluded unless you pass
  `--include-tests` or set `LYNX_INCLUDE_TESTS=1`.
- Binary files are skipped; only valid UTF-8 text is indexed.
- A failed parse of one file never fails the whole index.

Add `.lynx/` to the repository's `.gitignore` if it is not already ignored
globally:

```bash
echo '.lynx/' >> .gitignore
```

## 3. Query

```bash
# Ranked symbols (BM25 + embeddings, fused)
lx search "authentication middleware"

# Exact coordinates for a symbol; deterministic
lx resolve ValidateToken

# Structural edges incident to a symbol
lx relations auth::AuthService::Login

# Token-bounded context package as JSON
lx context "token refresh" --token-budget 2048
```

Example output shape for `lx resolve`:

```text
auth::AuthService::Login
  kind      Method
  language  Go
  file      internal/auth/service.go
  range     lines 42-67, bytes 1180-2043
  hash      9f2c…
```

`lx search --explain` prints the per-result lexical rank, semantic rank, fusion
score, definition boost, and final score — useful when a result is not what you
expected.

Retrieval commands **hydrate** the index and never re-index. If the index is
missing, they fail with an instruction to run `lx index`. If it is stale, they
warn on stderr and still answer.

## 4. Connect an agent (MCP)

Lynx ships one stdio MCP server. Any MCP client can use it; there is no SDK and
no per-client code path.

```bash
lx mcp                  # serve the enclosing git toplevel of the CWD
lx mcp /path/to/repo    # pin the workspace explicitly
lx mcp --check          # protocol smoke test, no indexing, then exit
```

The minimal client configuration is:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

For client-specific files (Claude Desktop, Claude Code, Cursor, VS Code,
OpenCode, Cline/Windsurf, Zed, Gemini CLI), see [mcp.md](mcp.md).

Notes:

- `lx mcp` hydrates an existing index. If the workspace has never been indexed,
  it builds the index once at startup, then serves queries.
- Use a project-scoped configuration so the server resolves the repository you
  are working in.
- If the client does not inherit your shell `PATH` (common for GUI apps),
  replace `"command": "lx"` with the absolute path printed by `lx doctor`
  (`Install …`).

## 5. Upgrade

Reinstall with the same channel you used. After upgrading, refresh the index if
`lx status` reports `incompatible`, `corrupt`, or a format change:

```bash
lx index --force
```

`lx index` increments the index in place when the format is compatible. A stale
index still answers queries; it does not automatically reflect new commits until
you re-index.

## 6. Uninstall

The installer has no uninstaller because it only writes one binary. Remove it
directly:

```bash
rm "$(command -v lx)"          # or: rm "$HOME/.local/bin/lx"
```

Per-repository indexes are not removed automatically:

```bash
rm -rf /path/to/repo/.lynx
```

The embedding-model cache is shared by all repositories:

```bash
rm -rf "$HOME/.cache/lynx/fastembed"
```

If you installed with Cargo, use `cargo uninstall pizen-lynx`.

## Next steps

- [Concepts](concepts.md) — what a symbol, evidence item, and relation mean.
- [CLI reference](cli.md) — every command and flag.
- [MCP reference](mcp.md) — tools and client configuration.
- [Troubleshooting](troubleshooting.md) — when something does not work.
- [Limits and measured behaviour](../README.md#resource-and-support-envelope) —
  what Lynx does and does not do well.
