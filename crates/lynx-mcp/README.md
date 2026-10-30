# pizen-lynx-mcp

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. It carries no stability guarantee.

The Model Context Protocol transport for [Lynx](https://github.com/PizenLabs/lynx).

It implements newline-delimited JSON-RPC 2.0 over stdio and exposes eight
consumer-agnostic tools: `search`, `resolve`, `resolve_all`, `inspect`,
`relations`, `similar`, `context`, and `index_status`. It owns session
bootstrap, request routing, and the protocol smoke check.

The canonical entry point is `lx mcp`; this crate declares no binary of its own.
Protocol frames go to stdout, diagnostics to stderr.

- MCP documentation: <https://github.com/PizenLabs/lynx/blob/main/docs/mcp.md>
- Compatibility matrix: <https://github.com/PizenLabs/lynx/blob/main/docs/architecture/mcp-compatibility.md>

## License

MIT — see [LICENSE](LICENSE).
