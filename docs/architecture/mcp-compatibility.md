# Lynx MCP Compatibility

This document records how independent MCP consumers talk to Lynx, the exact
configuration each needs, and the validation results observed.

Status vocabulary: **PASS** (verified with recorded evidence), **CONDITIONAL**
(wired and documented, but not executed in this environment), **FAIL**,
**UNSUPPORTED** (not claimed).

All consumers use the same canonical server, `lx mcp`, and the standard stdio
transport. No consumer-specific logic exists in Lynx.

## Contract under test

| Property | Value |
|---|---|
| Transport | newline-delimited JSON-RPC 2.0 over stdio |
| Protocol version | `2024-11-05` |
| Tools | `search`, `resolve`, `resolve_all`, `inspect`, `relations`, `similar`, `context`, `index_status` |
| Diagnostics | stderr only; stdout carries frames only |
| Shutdown | server exits 0 on stdin EOF |

## Validation record

### Independent stdio consumer (`tests/distribution/mcp_client.py`)

A purpose-built, protocol-only client that knows nothing about Lynx internals.
It spawns the **installed** binary outside the development checkout, performs
the handshake, discovers tools, queries the repository, and shuts the server
down. Run as part of `tests/distribution/clean_machine_test.sh`.

| Step | Result |
|---|---|
| Spawn `lx mcp` (cwd = arbitrary indexed repo) | PASS |
| `initialize` | PASS — `protocolVersion 2024-11-05`, `serverInfo.lynx-mcp 0.1.4` |
| `tools/list` | PASS — exactly the eight canonical tools, in stable order |
| `tools/call search` | PASS — returned ranked evidence; top hit `auth::Token` |
| `tools/call resolve` | PASS — resolved `auth::ValidateToken` |
| `tools/call context` | PASS — bounded package (`estimated_tokens` 210) |
| `tools/call index_status` | PASS — `file_count` 3 |
| Clean shutdown (stdin EOF) | PASS — exit 0 |
| `initialize` latency | ≈130 ms (loads the embedding model) |
| First query latency | ≈5 ms |
| Error behavior | malformed frames → JSON-RPC `-32700`, server continues; unknown tool → `-32602`; tool failure → `isError: true` with a message (covered by the crate test suite) |

**Overall: PASS** on macOS arm64.

### Named consumers

Configuration for each is documented in [`../mcp.md`](../mcp.md). The formats
for Claude Desktop, VS Code (Copilot), and OpenCode were verified against those
products' current official documentation during this phase. Live handshakes were
**not** executed in this environment (no consumer installed), so each is
**CONDITIONAL** until run.

| Consumer | Config surface | Config shape | Live handshake |
|---|---|---|---|
| Claude Desktop | `claude_desktop_config.json` | `mcpServers` / `command` + `args` | CONDITIONAL |
| Claude Code | `.mcp.json` or `claude mcp add` | `mcpServers` | CONDITIONAL |
| Cursor | `.cursor/mcp.json` / `~/.cursor/mcp.json` | `mcpServers` | CONDITIONAL |
| VS Code (Copilot) | `.mcp.json` (portable) / `.vscode/mcp.json` | `mcpServers` / `servers` | CONDITIONAL |
| OpenCode | `opencode.json` | `mcp` / `type: local` / `command: [...]` | CONDITIONAL |
| Cline / Windsurf | consumer config | `mcpServers` | CONDITIONAL |
| Zed | `settings.json` | `context_servers` | CONDITIONAL |
| Gemini CLI | `~/.gemini/settings.json` | `mcpServers` | CONDITIONAL |

There is no FAIL and no UNSUPPORTED consumer: every consumer that speaks
standard stdio MCP with a `command`/`args` (or equivalent) shape is supported by
construction, and the independent client exercises exactly that contract. The
CONDITIONAL status is a statement about what has been *run here*, not about
compatibility.

## Why configuration is documentation, not code

Auto-detecting consumer configs and editing them is fragile (different files,
schemas, scopes, and merge semantics per consumer) and would risk silently
mutating a user's configuration. `lx mcp setup` was evaluated and **not
implemented**; the exact snippets in [`../mcp.md`](../mcp.md) are the supported
path. See [`distribution-design.md`](distribution-design.md) §8.

## Re-running the consumer validation

```bash
cargo build --release -p pizen-lynx
tests/distribution/clean_machine_test.sh
```

The MCP stage prints a JSON summary (protocol, server, tools, latencies, top
result, shutdown). To drive a specific consumer, add the snippet from
[`../mcp.md`](../mcp.md) and confirm tool discovery and a `search` call.
