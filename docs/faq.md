# FAQ

### What is Lynx, in one sentence?

A local engine that turns a repository into stable symbols, coordinates,
relations, and embeddings, and serves them to tools and agents over a CLI and
the Model Context Protocol.

### Is Lynx an AI agent or a coding assistant?

No. It does not plan, reason, edit, or decide. It discovers evidence and returns
it. The consumer — an agent, IDE, script, or person — decides what to do.

### Does Lynx call an LLM or a cloud service?

No. Parsing, indexing, retrieval, ranking, and context assembly run locally. The
embedding model runs through ONNX Runtime on the CPU. The only network access is
the one-time model download on first use. There is no telemetry.

### Does Lynx need a GPU?

No. The embedding model is small and runs on the CPU.

### Does it need Git?

No. Git is optional. When the workspace is a Git repository, Lynx records the
commit, working-tree dirtiness, and a content hash in each snapshot, and
`lx status` can report staleness relative to `HEAD`.

### Which languages are supported?

Symbol extraction and intra-file relations: Rust, Go, TypeScript, JavaScript,
and Python. Markdown, YAML, JSON, TOML, and unknown UTF-8 text are indexed as
file/module documents without relations.

### Are relations cross-file?

No. Every relation is intra-file. Lynx does not link a call in one file to a
definition in another. This is a deliberate product boundary, not a missing
configuration.

### Where is the index stored?

`<workspace>/.lynx`, containing a SQLite graph/vector database and a Tantivy
lexical index. Add `.lynx/` to `.gitignore` unless your project already ignores
it.

### Does Lynx re-index on every query?

No. `lx index` writes the index. `search`, `resolve`, `relations`, `context`,
and `status` hydrate it and never re-index. `lx mcp` builds it once at startup
only when the workspace has never been indexed.

### Can I delete `.lynx` safely?

Yes. It is a derived artifact. `lx index` rebuilds it. Deleting it while a
process is using it may make that process fail; stop the process first.

### Does it work with monorepos?

Yes, with one index per workspace root. Index the root you want to query. If
different services should have separate indexes, index each service directory
separately and pass the path explicitly to `lx mcp`.

### How large a repository can it handle?

Up to the measured envelope documented in the
[product report](architecture/final-product-report.md). Memory does not grow
with repository size at query time, but semantic search is an exact scan over
all symbols, so latency grows with the number of symbols. See the
[limits](concepts.md#deliberate-limits).

### Why is a file missing from the index?

It is probably in an excluded directory, under a test/mock/generated path, over
8 MiB, binary, invalid UTF-8, or a symlink. See
[troubleshooting](troubleshooting.md#a-file-i-expected-is-not-indexed).

### Can I configure which files are excluded?

Not per project yet. The excluded directory segments and the 8 MiB file cap are
fixed. Use `--include-tests` to include test, mock, and generated paths, and
index a narrower directory if needed.

### Does it support Windows or macOS Intel?

No. Supported targets are macOS arm64 and Linux x86_64/arm64. macOS Intel cannot
build the embedding stack because upstream ONNX Runtime no longer publishes an
x86_64 macOS build.

### Which MCP clients work?

Any client that supports stdio MCP. The [MCP reference](mcp.md) includes
configuration for Claude Desktop, Claude Code, Cursor, VS Code/Copilot,
OpenCode, Cline/Windsurf, Zed, and Gemini CLI.

### Can multiple agents share one index?

Yes, as readers. Each client launches its own `lx mcp` process over the same
workspace and index. Avoid running `lx index` while a server is serving that
workspace; index first, then start or restart the server.

### Can I use Lynx as a Rust library?

Not as a supported API. The only supported surfaces are the `lx` binary and the
`lx mcp` server. The internal `pizen-lynx-*` crates are published because Cargo
requires them for `cargo install pizen-lynx`; they carry no stability guarantee.

### Why is the top search result not the symbol I meant?

Semantic top-1 is representation-limited. Use `lx resolve` for known names,
`--mode lexical` for exact terms, and `--explain` to see the ranking. See
[query quality](troubleshooting.md#query-quality).

### Are results deterministic?

Structural lookups (`resolve`, `relations`) are deterministic. Hybrid ranking is
deterministic for the same index, query, build, and platform. Semantic scores
depend on the embedding model and ONNX Runtime, so do not assume bit-identical
ordering across versions.

### How do I update Lynx?

Reinstall with the same channel, then run `lx status`. If the index is reported
`incompatible` or `corrupt`, run `lx index --force`. A stale index still answers
queries until you refresh it with `lx index`.

The installer and crates.io are separate channels: the installer follows the
newest stable GitHub Release, while `cargo install` follows crates.io. If the
installer gives you an older version than you expect, use
`cargo install pizen-lynx` or pin the release with `LYNX_VERSION`.

### How do I uninstall?

Remove the `lx` binary, any per-repository `.lynx` directories you no longer
want, and `$HOME/.cache/lynx/fastembed` if you want to reclaim the model space.
See [getting started](getting-started.md#6-uninstall).

### What license is Lynx under?

MIT. See [LICENSE](../LICENSE).

### How do I report a vulnerability?

Privately, through GitHub Security Advisories. See [SECURITY.md](../SECURITY.md).
Do not open a public issue.
