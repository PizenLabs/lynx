# pizen-lynx-parser

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. It carries no stability guarantee.

The source parser for [Lynx](https://github.com/PizenLabs/lynx). It routes a
file by extension to a tree-sitter adapter and produces frozen protocol
primitives:

```text
source -> SymbolIdentity[] + intra-file Relation[]
```

Grammar-backed adapters cover Rust, Go, TypeScript, JavaScript, and Python and
report `CapabilityLevel::L3Structural`. Markdown, YAML, JSON, TOML, and unknown
text use a grammar-less adapter at `L1Parsed` and emit no relations.

Relations are intentionally **intra-file**: an edge exists only when both
endpoints were extracted from the same source file.

- Architecture: <https://github.com/PizenLabs/lynx/blob/main/docs/architecture/final-product-audit.md>
- Product documentation: <https://github.com/PizenLabs/lynx#readme>

## License

MIT — see [LICENSE](LICENSE).
