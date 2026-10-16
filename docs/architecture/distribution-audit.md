# Lynx Distribution & Onboarding Audit

Status: **pre-implementation audit** (phase 2 of the distribution pass).
This document describes the repository as it exists *before* the distribution
layer was built. It is the frozen baseline the design in
[`distribution-design.md`](distribution-design.md) responds to.

Audit method: direct inspection of the repository at commit `4491634`
(`feature/install` branch), plus a local build of the release artifact on the
audit host (macOS arm64, `rustc 1.98.1`). No network-only facts are asserted.

---

## 1. Repository layout

```text
Cargo.toml                     Cargo workspace (resolver = 2, 7 members)
Cargo.lock                     single workspace lockfile
Makefile                       fmt / check / clippy / test convenience targets
README.md                      product + install + CLI + MCP docs
LICENSE                        MIT
crates/
  lynx-protocol/               frozen contracts (no I/O)
  lynx-parser/                 source -> SymbolIdentity + intra-file Relation
  lynx-embed/                  text -> vector (VectorProvider seam; FastEmbed)
  lynx-storage/                SQLite (graph/vectors) + Tantivy (lexical)
  lynx-core/                   Engine: discovery, indexing, retrieval, ranking
  lynx-mcp/                    stdio JSON-RPC transport + session lifecycle
  lynx-cli/                    `lx` command-line transport
benchmarks/                    reproducible benchmark scripts + reports
docs/architecture/             design, audit, and readiness documents
testdata/                      parser/engine fixtures
.github/workflows/ci.yml       lint + test (single platform)
.github/dependabot.yml         dependency updates
```

Only one workspace crate produces the user-facing CLI binary; the rest are
library crates plus one auxiliary MCP binary.

## 2. Build process (current)

- `cargo build --release -p pizen-lynx` builds the `lx` binary.
- `[profile.release]` is already tuned for distribution ships: `opt-level = 3`,
  `lto = true`, `codegen-units = 1`, `strip = true`.
- No cross-compilation configuration, no target matrix, no `[package.metadata]`
  release hints, no vendored model, no build script that fetches artifacts at
  build time.
- Local host is macOS arm64; the only installed Rust target is
  `aarch64-apple-darwin`. Linux and macOS x86_64 targets are **not** configured
  or verified anywhere in the repository.

Measured on the audit host (release build):

| Item | Value |
|---|---|
| `target/release/lx` size | ~32.5 MB (unstripped build present in `target/`; the profile requests `strip`) |
| Workspace members | 7 |
| Rust edition | 2021 |

## 3. Release process (current)

- **There is no release process.** CI (`.github/workflows/ci.yml`) runs only:
  `cargo fmt --all -- --check`, `cargo check`, `cargo clippy --all-targets --
  -D warnings`, and `cargo test` — all on `ubuntu-latest`, all from source.
- Git tags exist (`v0.0.1`, `v0.0.2`, `v0.0.3`, `v0.1.1`, `v0.1.2`, `v0.1.3`) but
  **no tag has any release assets**. There is no GitHub Release, no archive, no
  checksum, and no signature anywhere in the repository or its history.
- There is no `.github/workflows/release.yml`, no `cargo dist`/`cargo-release`
  configuration, and no publish workflow for crates.io.

## 4. Version source (current)

Single source of truth: `Cargo.toml` → `[workspace.package] version = "0.1.4"`.

- The CLI exposes it through clap's `#[command(version)]`, which reads
  `CARGO_PKG_VERSION` → `lx --version` prints `lx 0.1.4`.
- The MCP `initialize` response reports `serverInfo.version` from
  `env!("CARGO_PKG_VERSION")` → `0.1.4`.
- **Discrepancy:** the newest git tag is `v0.1.3`; the workspace declares
  `0.1.4`. The version has moved ahead of the last tag without a release.
- Crate versions are split: library members are pinned at `0.1.1` in
  `[workspace.dependencies]` while the workspace package version is `0.1.4`.
  This is harmless for a binary distribution but is a release-hygiene smell.

Verified locally: `lx --version` and `lx --help` complete **without** loading the
embedding model (clap parses and prints before any `Engine` is constructed).

## 5. Artifact naming (current)

None. There is no artifact scheme, no checksum file, and no fixed naming
convention. Consumers currently obtain Lynx with:

```bash
cargo install pizen-lynx      # compiles from source, requires a Rust toolchain
cargo build --release -p pizen-lynx
```

The published-crate name is `pizen-lynx`; the produced binary is `lx`. A second
binary, `pizen-lynx-mcp`, is produced by the `pizen-lynx-mcp` crate.

## 6. Supported OS / architectures (current)

The repository declares **no** binary support matrix. In practice:

| Platform | Declared support | Verified |
|---|---|---|
| macOS arm64 | implicit (dev host) | locally, from source |
| macOS x86_64 | none | never |
| Linux x86_64 | none | never (CI compiles/tests only) |
| Linux arm64 | none | never |
| Windows | none | never |

CI compiles and tests on `ubuntu-latest` x86_64 only. Compilation is not
distribution validation: nothing tests a *released binary* on a *clean machine*.

## 7. MCP startup contract (current)

- Canonical entrypoint: `lx mcp` (in-process, in `crates/lynx-cli`). A standalone
  `pizen-lynx-mcp [WORKSPACE_ROOT]` binary also exists.
- Transport: newline-delimited JSON-RPC 2.0 over stdio. Diagnostics go to
  stderr; stdout carries protocol frames only.
- Protocol version advertised: `2024-11-05`.
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`, plus a legacy
  direct-method form for each tool name.
- Tool surface (exactly eight, consumer-agnostic): `search`, `resolve`,
  `resolve_all`, `inspect`, `relations`, `similar`, `context`, `index_status`.
- Lifecycle: `Session::open` **hydrates** `<workspace>/.lynx`; it **builds once
  at startup if the index is absent**. Building loads the ~250 MB FastEmbed ONNX
  model. Hydration is `O(1)` and loads no repository source.
- Workspace root resolution: enclosing git toplevel of the CWD, else the CWD.
- There is no `lx mcp --check` and no protocol smoke test in the CLI.

## 8. Installation assumptions (current)

- Requires a **Rust toolchain** (`cargo install` / `cargo build`). This violates
  the target "install without another runtime".
- No installer script exists. No `install.sh`, no official `curl | sh` bootstrap.
- No detection of an existing installation, no PATH guidance, no atomic replace.
- Embedding model weights are **not** shipped in the binary. On first
  index/build the FastEmbed provider downloads them to
  `$HOME/.cache/lynx/fastembed` (or `$TMPDIR/lynx-fastembed` when `HOME` is
  unset). This means the *first index* requires network access and a writable
  cache directory.

## 9. Configuration assumptions (current)

- Repository artifact: `<repo>/.lynx/` (`graph.sqlite3`, `lexical/`, etc.).
  `.lynx` is gitignored in this repository and is Lynx-owned infrastructure.
  **No** `.izen`, `.ohgo`, or `.pizen` markers exist or are required.
- Model cache: `$HOME/.cache/lynx/fastembed`.
- Env vars: `LYNX_INCLUDE_TESTS=1` (standalone MCP binary only). No other magic.
- No user-global configuration file, no config directory, no dotfile.
- Environment variables consumed anywhere: `HOME`, `TMPDIR` (via
  `std::env::temp_dir`), and `LYNX_INCLUDE_TESTS`. Nothing PizenLabs-specific.

## 10. Security posture (current)

| Risk | Present today | Notes |
|---|---|---|
| No artifact integrity check | **Yes** | No checksum or signature exists to verify. |
| No HTTPS-only enforcement | n/a | There is no downloader to enforce it. |
| Source-build trust | **Yes** | `cargo install` fetches and compiles crates from the registry/toolchain. |
| First-run model download | **Yes** | Weights fetched over the network at first index; trust-on-first-use, no pinning in the installer. |
| Telemetry | **No** | The codebase contains no telemetry; none may be added. |
| Background daemon | **No** | No service, daemon, or auto-updater exists. |
| Shell-config mutation | **No** | Nothing writes to shell startup files. |
| Arbitrary RCE in installer | n/a | No installer exists yet. |
| PizenLabs runtime coupling | **No** | Engine and transports contain no PizenLabs/Lea/ohgo logic. Naming only. |

Repository-relative safety properties already hold and must be preserved:
`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`, no background threads per
query, index writer is the only mutator, and nothing about querying mutates the
workspace.

## 11. Gaps between current state and one-command installation

Ordered by impact on the release target:

1. **No prebuilt binaries.** A new user must install Rust and compile for
   minutes. Target: native prebuilt archives per platform.
2. **No release automation.** No target matrix, no archives, no checksums, no
   published assets. Target: a release workflow that builds and publishes all
   supported targets with `SHA256SUMS`.
3. **No installer.** Target: a small, auditable POSIX `install.sh` performing
   detect → download → verify → unpack → atomic install.
4. **No `lx doctor`.** No single command proves an installation is healthy and
   the index is usable. Target: a fast, model-free diagnostic.
5. **No `lx mcp --check`.** No lightweight, non-indexing proof that the MCP
   server starts, initializes, advertises tools, and terminates.
6. **No support matrix.** Platforms are neither declared nor tested. Target: a
   deterministic, *tested* matrix with honest status per platform.
7. **README install path is cargo-only.** The first five minutes assume a Rust
   toolchain and source checkout. Target: one-command install and a first-
   repository workflow before any architecture content.
8. **No consumer guidance.** No documented stdio MCP configuration for real
   agents, and no record of an independent consumer actually working.
9. **No distribution tests.** Nothing exercises the installer, the installed
   binary outside a checkout, or clean-machine assumptions.
10. **Version/tag drift.** Workspace `0.1.4` vs. tag `v0.1.3`; no artifact tied
    to the version. Target: version → tag → artifacts → checksums as one chain.

## 12. Scope guard for this phase

Audited and confirmed **in scope** for the distribution layer: installer,
release workflow, artifact scheme, checksums, `lx doctor`, `lx mcp --check`,
consumer documentation, distribution tests, README/docs, and validation reports.

Audited and confirmed **out of scope** (hard stop): the indexing engine,
retrieval/ranking algorithms, persistence architecture, MCP protocol/tool
surface, embedding model, and any agentic/daemon/telemetry behavior. No
consumer-specific logic may enter `lynx-core`; consumer differences live only in
documentation, never in the core.
