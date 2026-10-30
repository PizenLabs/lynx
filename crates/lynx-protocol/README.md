# pizen-lynx-protocol

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. It carries no stability guarantee.

The frozen domain contracts shared by every [Lynx](https://github.com/PizenLabs/lynx)
crate:

- `SymbolIdentity`, `SymbolKind`, `Language`
- `Evidence`, `SourceRange`, `RetrievalMode`
- `Relation`, `RelationKind`
- `Snapshot`
- `ContextPackage`, `CapabilityLevel`

The crate has no I/O and no business logic. Its serialized form **is** the wire
contract: enums serialize as their identifier verbatim (`"Rust"`,
`"L3Structural"`), and renaming or removing a serialized field is a breaking
change.

- Architecture: <https://github.com/PizenLabs/lynx/blob/main/docs/architecture/README.md>
- Repository: <https://github.com/PizenLabs/lynx>

## License

MIT — see [LICENSE](LICENSE).
