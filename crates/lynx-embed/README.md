# pizen-lynx-embed

> **Internal implementation crate — not a public API.** This crate is published
> to crates.io only because Cargo requires a registry-resolvable dependency
> graph for `cargo install pizen-lynx`. It carries no stability guarantee.

The embedding provider for [Lynx](https://github.com/PizenLabs/lynx).

It defines the `VectorProvider` seam — query embedding, batch embedding, and
dimension — and ships the local FastEmbed implementation backed by
`bge-small-en-v1.5` (384 dimensions) through ONNX Runtime. Model weights are
cached under `$HOME/.cache/lynx/fastembed` by the transports; no cloud API and no
GPU are required.

The crate also provides an `UnavailableProvider` used by structural commands
that must not load the model.

- Product documentation: <https://github.com/PizenLabs/lynx/blob/main/docs/concepts.md>
- Repository: <https://github.com/PizenLabs/lynx>

## License

MIT — see [LICENSE](LICENSE).
