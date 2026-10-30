# pizen-lynx-core

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. Depend on the `pizen-lynx` product
> package, or use the `lx` binary and its MCP server, instead.

The core engine for [Lynx](https://github.com/PizenLabs/lynx): discovery,
incremental indexing, retrieval, reciprocal-rank fusion, deterministic
resolution, relation traversal, and context-package compilation.

It ties together the frozen protocol types, the parser adapters, the dual
SQLite/Tantivy storage substrate, and a `VectorProvider` embedding seam. The
transports (`pizen-lynx`, `pizen-lynx-mcp`) contain no product logic.

- Architecture: <https://github.com/PizenLabs/lynx/blob/main/docs/architecture/README.md>
- Product documentation: <https://github.com/PizenLabs/lynx#readme>

## License

MIT — see [LICENSE](LICENSE).
