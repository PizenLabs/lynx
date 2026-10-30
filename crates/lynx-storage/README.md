# pizen-lynx-storage

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. It carries no stability guarantee.

The persistence layer for [Lynx](https://github.com/PizenLabs/lynx):

- **SQLite** (`rusqlite`) for symbol identity, structural relations, vectors,
  snapshots, and metadata;
- **Tantivy** for BM25 lexical documents and source snippets.

Both substrates live under `<workspace>/.lynx` and are opened as one
`DualStorage` handle. The layer owns durability and index format, not retrieval
policy.

- Architecture: <https://github.com/PizenLabs/lynx/blob/main/docs/architecture/final-product-audit.md>
- Product documentation: <https://github.com/PizenLabs/lynx#readme>

## License

MIT — see [LICENSE](LICENSE).
