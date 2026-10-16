# Lynx MCP server

Lynx ships **one** canonical Model Context Protocol server. Any MCP client can
use it; there is no Lynx-specific SDK and no per-consumer code path.

```bash
lx mcp                 # stdio server for the git toplevel of the current directory
lx mcp /path/to/repo   # explicit workspace
lx mcp --check         # fast protocol smoke test, no indexing, then exit
```

- **Transport.** Newline-delimited JSON-RPC 2.0 over stdio. Diagnostics go to
  stderr; stdout carries protocol frames only.
- **Protocol version.** `2024-11-05`.
- **Lifecycle.** Hydrates `<workspace>/.lynx` at startup; builds it once if
  absent. Queries never re-index.
- **Workspace.** The enclosing git toplevel of the launch directory, else the
  launch directory. Pass `PATH` explicitly if your consumer launches from an
  ambiguous directory.

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `search` | `query`, `mode?`, `limit?`, `timeout_ms?` | Ranked `Evidence` (hybrid RRF) |
| `resolve` | `name` | The deterministic best symbol, or `null` |
| `resolve_all` | `name` | Every symbol matching a name (ambiguity) |
| `inspect` | `symbol_hash` | Full evidence for one symbol |
| `relations` | `symbol_hash`, `kind?` | Structural edges incident to a symbol |
| `similar` | `symbol_hash`, `timeout_ms?` | Embedding-similarity neighbors |
| `context` | `query`, `token_budget?`, `timeout_ms?` | Budget-bounded `ContextPackage` |
| `index_status` | `path?`, `check_freshness?` | Counts, provenance, freshness |

The surface is consumer-agnostic and is not extended for onboarding.

## Consumer configuration

Every consumer below uses the same minimal invocation: start the installed `lx`
with the `mcp` argument. **Lynx does not edit these files for you** — copy the
snippet for your consumer. Prefer a project-local configuration so the server's
workspace resolves to the repository you are working in; use a global
configuration only if you pass an explicit `PATH`.

If `lx` is not on the consumer's `PATH`, replace `"command": "lx"` with the
absolute path printed by `lx doctor` (`Install …`).

### Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

### Claude Code

Project-scoped `.mcp.json` at the repository root:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

Or add it from the CLI: `claude mcp add lynx -- lx mcp`.

### Cursor

Project `.cursor/mcp.json`, or global `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

### VS Code (GitHub Copilot)

Portable workspace config `.mcp.json` at the repository root:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

VS Code-specific workspace config `.vscode/mcp.json` uses a `servers` object
instead:

```json
{
  "servers": {
    "lynx": { "type": "stdio", "command": "lx", "args": ["mcp"] }
  }
}
```

### OpenCode

`opencode.json` (project or global). OpenCode's `command` is an array:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "lynx": { "type": "local", "command": ["lx", "mcp"], "enabled": true }
  }
}
```

### Cline / Windsurf / other `mcpServers` clients

Most stdio MCP clients accept the same `mcpServers` shape:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

### Zed

Zed uses `context_servers` in `settings.json`; confirm against your Zed version:

```json
{
  "context_servers": {
    "lynx": { "command": { "path": "lx", "args": ["mcp"] } }
  }
}
```

### Gemini CLI

`~/.gemini/settings.json`; confirm against your Gemini CLI version:

```json
{
  "mcpServers": {
    "lynx": { "command": "lx", "args": ["mcp"] }
  }
}
```

## Verifying a consumer

1. `lx doctor` → `Status READY` in the repository.
2. Start the consumer and confirm the eight tools are discovered.
3. Ask it to `search` for a symbol you know exists, then `resolve` it.

See [`architecture/mcp-compatibility.md`](architecture/mcp-compatibility.md) for
the validation matrix and recorded results.
