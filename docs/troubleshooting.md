# Troubleshooting

Start with the two commands that answer most questions:

```bash
lx --version
lx doctor
```

`lx doctor` reports the binary, version, platform, install path, workspace,
state path, permissions, repository detection, index state, capability level,
and the MCP contract. `Status READY` requires a fresh index and a passing MCP
check. Run it **from the repository you care about** — the workspace line tells
you which root Lynx resolved.

## Installation

### `lx: command not found`

The installer puts the binary in `$HOME/.local/bin`, which may not be on your
`PATH`. Add it for your shell:

```bash
# zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
# bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

Then restart the shell and run `lx --version`. Alternatively, install to a
directory already on `PATH`:

```bash
sh install.sh --dir "$HOME/bin"
```

### The installer fails on my platform

Supported targets are macOS arm64 and Linux x86_64/arm64. macOS Intel and
Windows are not supported. On an unsupported platform, build from source where
the native dependencies are available, or use a supported machine. macOS Intel
cannot build the embedding stack because upstream ONNX Runtime no longer
publishes an x86_64 macOS build.

The installer refuses prereleases by default. If you are intentionally testing
one, set `LYNX_ALLOW_PRERELEASE=1`. For a specific stable release:

```bash
LYNX_VERSION=v0.2.1 sh install.sh
```

If the download fails behind a proxy or a GitHub rate limit, retry later, use
`--download-only=DIR` to inspect the artifact, or install with
`cargo install pizen-lynx`. Do not disable checksum verification in production.

### The installer installed an older version than crates.io

The installer and crates.io are separate channels. The installer resolves the
newest stable **GitHub Release**; `cargo install` resolves the newest version on
**crates.io**. If the GitHub release is older than the crate you need:

```bash
cargo install pizen-lynx   # newest crates.io release
```

Or pin a published GitHub release with `LYNX_VERSION=vX.Y.Z`; only tags that
appear on the releases page can be installed.

### `doctor` says `Status NOT READY`

Read the `Next steps` lines. The common cases:

| Symptom | Fix |
|---|---|
| `Index missing (0 symbols)` | `lx index` |
| `Index stale` | `lx index` |
| `Index incompatible` or `corrupt` | `lx index --force` |
| `Permissions` not OK | Make the workspace (or `.lynx`) writable, or index a writable copy. |
| `MCP` not OK | Run `lx mcp --check` and see the MCP section below. |

A freshly installed binary in an unindexed repository reporting `NOT READY` is
expected, not an installation failure.

## Indexing

### `workspace … is not indexed (no …/.lynx); run lx index first`

A retrieval command was run before the workspace was indexed. Run:

```bash
lx index
```

`lx mcp` is the exception: it builds the index once at startup when it is
absent. `lx search`/`resolve`/`relations`/`context` never index.

### `index at … is empty; re-run lx index`

The `.lynx` directory exists but contains no symbols. This can happen after an
interrupted or failed build. Rebuild:

```bash
lx index --force
```

### `index is stale` warning

The indexed commit differs from the workspace `HEAD`. Queries still answer from
the stored index. Refresh it:

```bash
lx index
```

### The index says `incompatible` or `corrupt`

A format change or a damaged substrate. The recovery path is always a clean
rebuild:

```bash
lx index --force
```

`--force` deletes `.lynx` first, so no stale row or segment can survive.

### Indexing is slow or uses a lot of memory

The first index parses every eligible file and embeds every symbol. The
embedding model itself is roughly 250 MB resident. Subsequent runs are
incremental and much cheaper.

If the first pass is too heavy:

- index from the repository root, not a directory containing vendored
  dependencies;
- keep generated and vendored trees out of the workspace (they are excluded by
  segment name, but not every project uses those names);
- files over 8 MiB are already skipped;
- index a smaller subdirectory explicitly if that is the unit you query.

There is no per-project ignore file yet; the excluded segments are fixed and
documented in [Concepts](concepts.md#what-is-excluded).

### A file I expected is not indexed

Check, in order:

1. Is it inside `.git`, `.lynx`, `.fastembed_cache`, `node_modules`, `vendor`,
   `target`, `build`, or `dist`? Those are always excluded.
2. Is it under `test`, `tests`, `mock`, `mocks`, or `generated`? Re-index with
   `--include-tests`.
3. Is it larger than 8 MiB, binary, or invalid UTF-8? It is skipped.
4. Is it a symlink? Symlinks are not followed.
5. Does it have a recognized extension? Unknown extensions become generic
   module documents at `L1Parsed`, not structural symbols.

### `lx index` indexed the wrong directory

`lx index [PATH]` canonicalizes `PATH` and writes `<PATH>/.lynx`. Retrieval
commands resolve the **workspace root** from the current directory (the
enclosing Git toplevel, otherwise the current directory). If you index a
subdirectory and then query from the repository root, the root has no index.

Run both commands from the same root, or pass the path you indexed to `lx mcp`:

```bash
lx index /path/to/repo
cd /path/to/repo && lx search "…"
```

### `lx index` fails with a permission error

Lynx needs to create and write `<workspace>/.lynx`. Fix ownership or ACLs, or
copy the repository to a writable location. Read-only source trees can be
queried only if the index directory is writable.

## Model and network

### The first index tries to download a model

Expected. The `bge-small-en-v1.5` embedding model is downloaded once into
`$HOME/.cache/lynx/fastembed` (or `$TMPDIR/lynx-fastembed` when `HOME` is unset)
on the first index, search, context, or `lx mcp` startup. After that, no network
access is needed.

If the download fails:

- check network access to the model host and any proxy configuration;
- ensure `$HOME` (or its cache subdirectory) is writable;
- retry `lx index`;
- as a temporary fallback, use the structural commands that never initialize
  the model: `lx resolve`, `lx relations`, `lx status`, `lx doctor`, and
  `lx --version`.

### Structural commands are fast but search is slow

`resolve`, `relations`, `status`, `doctor`, and `--version` load no embedding
model. `search` and `context` initialize it, even with `--mode lexical`:
lexical mode skips the query embedding and the vector scan, but the session
still constructs the provider. For exact identifiers, prefer `lx resolve` or
`lx search "ExactName" --mode lexical` and see the [bounds](concepts.md#bounds).

## Query quality

### A symbol I know exists is not in the results

Try the deterministic route first:

```bash
lx resolve ExactName
lx search "ExactName" --mode lexical
```

Check that the file is indexable (see above) and that the index is fresh. If the
symbol is in an excluded or test path, rebuild with `--include-tests`.

### The right symbol is not first

Semantic top-1 is representation-limited: the right symbol is usually in the
results, but a sibling can rank first. Inspect the ranking:

```bash
lx search "the query" --explain
```

The explanation prints lexical rank, semantic rank, raw RRF, definition boost,
and final score. A result missing from one channel, or carried only by a weak
semantic match, is visible there.

For a known name, `resolve` bypasses ranking entirely and is the right tool.

### `lx relations` returns nothing or misses a call

Relations are **intra-file**. A call from one file to another is not linked,
even when both symbols are indexed. The `relations` command reports only edges
observed inside the symbol's own file.

### `lx context` returns less than I asked for

The budget is enforced on the serialized package, and envelope overhead (query,
identities, relations) is reserved before snippets. Raise `--token-budget`, or
ask a narrower query. The package is ordered by retrieval rank; the first item
is not guaranteed to be the best answer.

## MCP

### The client does not list Lynx's tools

1. Run the protocol smoke test:

   ```bash
   lx mcp --check
   ```

2. Use the absolute binary path in the client configuration. GUI clients often
   do not inherit your shell `PATH`:

   ```bash
   lx doctor   # Install: /Users/you/.local/bin/lx
   ```

3. Use the correct configuration shape for the client. See
   [MCP reference](mcp.md); VS Code, OpenCode, and Zed differ from the generic
   `mcpServers` shape.
4. Restart the client after editing its configuration.
5. Check that nothing else writes to the server's stdout. Lynx keeps stdout for
   protocol frames and sends diagnostics to stderr; a wrapper script that prints
   to stdout will corrupt the stream.

### The client starts Lynx but every call fails

Usually a workspace problem:

- The server resolves the enclosing Git toplevel, not necessarily the directory
  you expect. Pin it explicitly: `lx mcp /path/to/repo`.
- The workspace may have no index. `lx mcp` builds one at startup, which can be
  slow; pre-index with `lx index` so the first call is fast.
- Check `lx doctor` from the same directory the client launches in.

### Two clients or an editor and a query at the same time

Queries are read-only and may run concurrently. Avoid running `lx index` against
a workspace while an MCP server is serving it; finish indexing, then restart the
server or let it re-hydrate. If a client holds a stale handle, restart the
client after re-indexing.

### The MCP server exits immediately

Run `lx mcp --check` and then `lx mcp` manually in a terminal. Startup errors are
written to stderr. A missing model cache or an unreadable workspace is reported
there; clients sometimes hide stderr.

## Platform and environment

### macOS Intel or Windows

Unsupported. See [Supported platforms](../README.md#supported-platforms).

### `HOME` is unset

The model cache falls back to the system temporary directory, which is usually
cleared on reboot. Set `HOME` or pre-populate the cache for reproducible
environments.

### I need fully offline operation

1. On a connected machine, run `lx index` once to populate
   `$HOME/.cache/lynx/fastembed` and the model cache.
2. Copy that directory to the offline machine at the same path.
3. Copy the repository and its `.lynx` directory, or re-index offline using the
   cached model.

No other network access is performed by queries.

## Reporting a problem

If none of the above applies, open a bug report with:

- `lx --version` and platform;
- install channel;
- the exact command and full output;
- `lx doctor` output;
- a minimal reproduction (repository, file, query, or MCP frame).

Remove secrets and private paths first. For vulnerabilities, use
[SECURITY.md](../SECURITY.md), never a public issue.
