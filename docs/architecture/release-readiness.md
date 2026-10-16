# Lynx Release Readiness

The release process, gate, and artifact matrix. See
[`distribution-design.md`](distribution-design.md) for the design and
[`distribution-release-report.md`](distribution-release-report.md) for the
validated results.

There are two distribution surfaces, both gated by the same tag:

1. **Prebuilt archives** (GitHub Releases) — consumed by `install.sh`.
2. **crates.io packages** — consumed by `cargo install pizen-lynx`.

## 1. Release chain

```text
bump [workspace.package] version + [workspace.dependencies] versions in Cargo.toml
    -> commit
    -> tag vX.Y.Z (must equal the version)
    -> .github/workflows/release.yml
         verify   : fmt / check / clippy / test
         guard    : tag == workspace version
         build    : native build + package + smoke + clean-machine, per target
         publish  : SHA256SUMS + GitHub Release assets
    -> (gated, manual) crates.io publish in dependency order
    -> install.sh resolves vX.Y.Z and verifies the checksum before installing
    -> cargo install pizen-lynx resolves the published 0.2.0 graph
```

The tag is the single release trigger. A tag that does not match the workspace
version fails the `version-guard` job, so version drift cannot be released.

## 2. Artifact matrix

| Target | Runner | Artifact | Checksum | Status |
|---|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | in `SHA256SUMS` | validated locally |
| macOS x86_64 | `macos-15-intel` | `lynx-vX.Y.Z-darwin-x86_64.tar.gz` | in `SHA256SUMS` | CI-native |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | in `SHA256SUMS` | CI-native |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | in `SHA256SUMS` | CI-native |
| Windows | — | — | — | **UNSUPPORTED** |

"CI-native" means the target is built and validated natively by the release
workflow but has not been executed in the environment where this phase was
developed. The release must not be cut until every matrix job is green.

## 3. crates.io package matrix (publish order)

| # | Package | Role | Stability contract |
|---|---|---|---|
| 1 | `pizen-lynx-protocol` | internal contract | none |
| 2 | `pizen-lynx-parser` | internal implementation | none |
| 3 | `pizen-lynx-embed` | internal implementation | none |
| 4 | `pizen-lynx-storage` | internal implementation | none |
| 5 | `pizen-lynx-core` | internal engine | none |
| 6 | `pizen-lynx-mcp` | internal implementation | none |
| 7 | `pizen-lynx` | **the product** | **public, versioned** |

Publishing order is forced by the intra-workspace dependency graph. A crate
cannot be packaged or published until its internal dependencies already exist on
the registry — this is a Cargo invariant, not a policy choice. Only
`pizen-lynx` is a public API; see
[`distribution-design.md`](distribution-design.md#13-public-surface-and-stability).

```bash
# Dry-run first, then publish in order (maintainer-run; not automated):
for p in pizen-lynx-protocol pizen-lynx-parser pizen-lynx-embed \
         pizen-lynx-storage pizen-lynx-core pizen-lynx-mcp pizen-lynx; do
    cargo publish --dry-run -p "$p"
done
for p in pizen-lynx-protocol pizen-lynx-parser pizen-lynx-embed \
         pizen-lynx-storage pizen-lynx-core pizen-lynx-mcp pizen-lynx; do
    cargo publish -p "$p"
done
```

## 4. Release gate

A release is cut only when all of the following hold:

- [ ] Tag `vX.Y.Z` equals `[workspace.package] version`.
- [ ] `[workspace.dependencies]` version requirements equal the release version.
- [ ] `verify` job green (fmt / check / clippy / test).
- [ ] Every `build` matrix job green — archive produced.
- [ ] Native `lx --version` and `lx mcp --check` green on every target.
- [ ] Native `clean_machine_test.sh` green on every target.
- [ ] `SHA256SUMS` generated from the published archives.
- [ ] Release assets present and downloadable.
- [ ] `install.sh` installs the release on at least one platform and reports the
      expected version.
- [ ] `tests/distribution/cargo_install_test.sh` green (registry install of
      `pizen-lynx`).
- [ ] Ordered `cargo publish` completed for all seven crates.
- [ ] `cargo install pizen-lynx` succeeds from a clean environment and
      `lx --version` matches the tag.

## 5. What the release workflow guarantees

- Archives are built natively on each target (no cross-compilation claims).
- Failed/skipped targets publish nothing: `release` needs all of `build`, and
  `SHA256SUMS` is generated after the build matrix completes.
- CI additionally runs the hermetic installer suite on Linux + macOS on every
  push and pull request (`ci.yml` → `distribution` job).
- crates.io publication is **not** automated: the boundary must be confirmed and
  the ordered publish run deliberately (see §6).

## 6. Reproduce locally

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cargo build --release -p pizen-lynx
make dist-test                 # installer suite + clean-machine validation
tests/distribution/cargo_install_test.sh   # registry install of pizen-lynx

# Exercise the installer against a synthetic release served over loopback:
tests/distribution/install_test.sh

# Exercise the exact published-install flow against the real binary:
tests/distribution/clean_machine_test.sh
```

## 7. Known limits

1. **Unverified-on-this-host platforms.** Only macOS arm64 was executed here.
   The other matrix targets are wired to native CI runners and are gated by the
   release job.
2. **`ubuntu-24.04-arm` availability.** arm64 GitHub-hosted runners depend on the
   repository's plan; if the runner is unavailable, the `build` job fails and no
   release is published (fail-closed, not fail-silent).
3. **First index needs network.** The embedding model is fetched once into
   `$HOME/.cache/lynx/fastembed`; a fully offline first index is not supported.
4. **macOS notarization/signing.** Not present; binaries are unsigned. Checksums
   cover transport integrity, not publisher authenticity.
5. **No Windows.** Explicitly unsupported and rejected by the installer.
6. **crates.io requires the ordered publish.** Because the workspace is not a
   single crate, publishing `pizen-lynx` requires the six internal crates to be
   published first; the hermetic registry test proves the graph installs without
   requiring credentials.
