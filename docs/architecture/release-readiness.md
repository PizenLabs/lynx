# Lynx Release Readiness

The release process, gate, and artifact matrix for the distribution phase. See
[`distribution-design.md`](distribution-design.md) for the design and
[`distribution-release-report.md`](distribution-release-report.md) for the
validated results.

## 1. Release chain

```text
bump [workspace.package] version in Cargo.toml
    -> commit
    -> tag vX.Y.Z (must equal the version)
    -> .github/workflows/release.yml
         verify   : fmt / check / clippy / test
         guard    : tag == workspace version
         build    : native build + package + smoke + clean-machine, per target
         publish  : SHA256SUMS + GitHub Release assets
    -> install.sh resolves vX.Y.Z and verifies the checksum before installing
```

The tag is the single release trigger. A tag that does not match the workspace
version fails the `version-guard` job, so version drift cannot be released.

## 2. Artifact matrix

| Target | Runner | Artifact | Checksum | Status |
|---|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | in `SHA256SUMS` | **PASS** (validated on this host) |
| macOS x86_64 | `macos-15-intel` | `lynx-vX.Y.Z-darwin-x86_64.tar.gz` | in `SHA256SUMS` | **CONDITIONAL** (CI-native; not run here) |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | in `SHA256SUMS` | **CONDITIONAL** (CI-native; not run here) |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | in `SHA256SUMS` | **CONDITIONAL** (CI-native; not run here) |
| Windows | — | — | — | **UNSUPPORTED** |

"CONDITIONAL" here means the target is built and validated natively by the
release workflow but has not been executed in the environment where this phase
was developed. The release must not be cut until every matrix job is green.

## 3. Release gate

A release is cut only when all of the following hold:

- [ ] Tag `vX.Y.Z` equals `[workspace.package] version`.
- [ ] `verify` job green (fmt / check / clippy / test).
- [ ] Every `build` matrix job green — archive produced.
- [ ] Native `lx --version` and `lx mcp --check` green on every target.
- [ ] Native `clean_machine_test.sh` green on every target.
- [ ] `SHA256SUMS` generated from the published archives.
- [ ] Release assets present and downloadable.
- [ ] `install.sh` installs the release on at least one platform and reports the
      expected version.

## 4. What the release workflow guarantees

- Archives are built natively on each target (no cross-compilation claims).
- Failed/skipped targets publish nothing: `release` needs all of `build`, and
  `SHA256SUMS` is generated after the build matrix completes.
- CI additionally runs the hermetic installer suite on Linux + macOS on every
  push and pull request (`ci.yml` → `distribution` job).

## 5. Reproduce locally

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cargo build --release -p pizen-lynx
make dist-test                 # installer suite + clean-machine validation

# Exercise the installer against a synthetic release served over loopback:
tests/distribution/install_test.sh

# Exercise the exact published-install flow against the real binary:
tests/distribution/clean_machine_test.sh
```

## 6. Known limits

1. **Unverified-on-this-host platforms.** Only macOS arm64 was executed here.
   The other matrix targets are wired to native CI runners and are gated by the
   release job.
2. **`ubuntu-24.04-arm` availability.** arm64 GitHub-hosted runners depend on the
   repository's plan; if the runner is unavailable, the `build` job fails and no
   release is published (fail-closed, not fail-silent).
3. **First index needs network.** The embedding model is fetched once into
   `$HOME/.cache/lynx/fastembed`; a fully offline first index is not supported.
4. **macOS notarization/signing.** Not present; binaries are unsigned. This is a
   distribution-integrity gap, not a runtime gap. Checksums cover transport
   integrity, not publisher authenticity.
5. **No Windows.** Explicitly unsupported and rejected by the installer.
