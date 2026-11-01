# Lynx Release Readiness

The release process, gate, and artifact matrix. See
[`distribution-design.md`](distribution-design.md) for the design and
[`distribution-release-report.md`](distribution-release-report.md) for the
validated results.

There are two distribution surfaces, both gated by the same tag:

1. **Prebuilt archives** (GitHub Releases) — consumed by `install.sh`.
2. **crates.io packages** — consumed by `cargo install pizen-lynx`.

The normative contract is [`release-contract.md`](release-contract.md).

## 0. Release state machine

The tag's shape decides the path. The rule lives once in
`scripts/release/classify_tag.sh`, invoked by the workflow and by the tests.

```text
TAG
 │
 ├── invalid ───────────────→ FAIL (classify job exits non-zero)
 │
 ├── prerelease ────────────→ VALIDATE ONLY → STOP
 │                             (build + smoke test; no artifacts, no Release)
 │
 └── stable ────────────────→ BUILD → VERIFY → RELEASE
```

| Tag | Kind | Result |
|---|---|---|
| `v1.0.1` | stable | build + checksums + GitHub Release |
| `v1.0.1-rc.1`, `v1.0.1-alpha.1`, `v1.0.1-beta.1` | prerelease | validation only; no artifacts, no Release |
| `v1`, `v1.0`, `v1.0.0foo`, `release-v1.0.0` | invalid | fail closed |

## 1. Release chain

```text
bump [workspace.package] version + [workspace.dependencies] versions in Cargo.toml
    -> commit
    -> tag vX.Y.Z (must equal the version)
    -> .github/workflows/release.yml
         classify : scripts/release/classify_tag.sh -> stable | prerelease | invalid
         verify   : fmt / check / clippy / test
         guard    : tag == resolved Cargo package version (check_versions.sh)
         build    : (stable only) cross-compile on ubuntu-latest + version
                    check + package + smoke/static validation, per target
         publish  : (stable only) SHA256SUMS from final artifacts + GitHub Release
    -> (gated, manual) crates.io publish in dependency order
    -> install.sh resolves vX.Y.Z and verifies the checksum before installing
    -> cargo install pizen-lynx resolves the published 0.2.0 graph
```

The tag is the single release trigger. A tag that does not match the resolved
package version fails the `version-guard` job; an invalid tag fails the
`classify` job; a prerelease tag runs validation only and can never reach
`build`/`release`. Version drift cannot be released.

## 2. Artifact matrix

| Target | Build host / engine | Artifact | Checksum | Policy |
|---|---|---|---|---|
| macOS arm64 | `ubuntu-latest` + osxcross | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | in `SHA256SUMS` | REQUIRED (cross-compiled) |
| Linux x86_64 | `ubuntu-latest` native | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | in `SHA256SUMS` | REQUIRED (native) |
| Linux arm64 | `ubuntu-latest` + GNU cross toolchain | `lynx-vX.Y.Z-linux-arm64.tar.gz` | in `SHA256SUMS` | REQUIRED (cross-compiled) |
| macOS x86_64 | — | — | — | **UNSUPPORTED** (no upstream ONNX Runtime build) |
| Windows | — | — | — | **UNSUPPORTED** (no upstream ONNX Runtime build) |
| Linux musl | — | — | — | **UNSUPPORTED** (no upstream ONNX Runtime build) |

Every target is built on the standard Linux runner. No macOS or Windows runner
is scheduled; non-Linux binaries are produced with cross toolchains.

A declared target that cannot be built fails the release (fail-closed); no
partial artifact set is published.

macOS Intel (x86_64) is deliberately absent: the embedding engine is backed by
ONNX Runtime (via `fastembed`/`ort`), and upstream ONNX Runtime no longer
publishes an x86_64 macOS build, so the target cannot be built at all. It is not
declared, `install.sh` rejects it explicitly, and no artifact is claimed.
Windows and musl are absent for the same reason: `ort-sys` publishes no MinGW or
musl ONNX Runtime binary.

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

- [ ] Tag classifies as **stable** (`scripts/release/classify_tag.sh`).
- [ ] Tag `vX.Y.Z` equals the resolved `pizen-lynx` package version.
- [ ] `[workspace.dependencies]` version requirements equal the release version.
- [ ] `verify` job green (fmt / check / clippy / test).
- [ ] Every `build` matrix job green — archive produced.
- [ ] Executable targets report the tag version: `lx --version` and
      `lx mcp --check` on `linux-x86_64` (native) and `linux-arm64` (QEMU).
- [ ] `darwin-arm64` embeds the tag version and is a 64-bit arm64 Mach-O binary.
- [ ] Each archive contains exactly `lx` at its root.
- [ ] `clean_machine_test.sh` green on `linux-x86_64`.
- [ ] `SHA256SUMS` generated from the published archives; every archive present.
- [ ] Release assets present and downloadable.
- [ ] `install.sh` installs the release on at least one platform and reports the
      expected version.
- [ ] `tests/distribution/release_test.sh` green (classification + wiring).
- [ ] `tests/distribution/cargo_install_test.sh` green (registry install of
      `pizen-lynx`, single `lx` binary).
- [ ] Ordered `cargo publish` completed for all seven crates.
- [ ] `cargo install pizen-lynx` succeeds from a clean environment and
      `lx --version` matches the tag.

A **prerelease** tag (`vX.Y.Z-pre`) cuts nothing: it runs the `verify` job and
the validation-only build, and stops. It never uploads an artifact or creates a
GitHub Release.

## 5. What the release workflow guarantees

- The tag is classified by one shared rule; a prerelease tag cannot reach the
  `build` or `release` jobs.
- Every job runs on `ubuntu-latest`; no macOS or Windows runner is scheduled.
- Non-Linux archives are cross-compiled on Linux: Linux arm64 with the GNU
  cross toolchain, macOS arm64 with osxcross.
- Runtime validation is bounded by Linux execution: `linux-x86_64` runs the
  clean-machine suite, `linux-arm64` runs `--version`/`mcp --check` under QEMU,
  and `darwin-arm64` is validated statically (version string and Mach-O arch).
- Failed/skipped targets publish nothing: `release` needs all of `build`, and
  `SHA256SUMS` is generated after the build matrix completes.
- Each archive contains exactly `lx`; the built binary reports the tag version.
- CI additionally runs the hermetic installer suite on Linux on every push and
  pull request (`ci.yml` → `distribution` job) and the release
  classification/version guards (`ci.yml` → `release` job).
- crates.io publication is **not** automated: the boundary must be confirmed and
  the ordered publish run deliberately (see §6).

## 6. Reproduce locally

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cargo build --release -p pizen-lynx
make dist-test                 # release classification + installer + clean-machine
tests/distribution/cargo_install_test.sh   # registry install of pizen-lynx

# Release classification and version guards (the workflow's own logic):
tests/distribution/release_test.sh

# Exercise the installer against a synthetic release served over loopback:
tests/distribution/install_test.sh

# Exercise the exact published-install flow against the real binary:
tests/distribution/clean_machine_test.sh
```

## 7. Known limits

1. **Cross-compiled, not natively executed.** `linux-x86_64` is executed
   natively. `linux-arm64` is executed under QEMU. `darwin-arm64` cannot be
   executed on a Linux runner; it is validated by tag/package version, embedded
   version string, and Mach-O architecture. The full clean-machine flow is
   exercised on `linux-x86_64` only.
2. **macOS Intel (x86_64) is unsupported.** The embedding engine is backed by
   ONNX Runtime, and no upstream x86_64 macOS build exists (neither the
   `fastembed`/`ort` prebuilt binaries nor Microsoft's own release assets), so
   the target cannot be built. It is not in the matrix and `install.sh` rejects
   it explicitly.
3. **No Windows or musl.** `ort-sys` publishes no MinGW or musl ONNX Runtime
   binary, so those targets cannot be built. They are not declared and the
   installer rejects them.
4. **First index needs network.** The embedding model is fetched once into
   `$HOME/.cache/lynx/fastembed`; a fully offline first index is not supported.
5. **macOS notarization/signing.** Not present; binaries are unsigned. Checksums
   cover transport integrity, not publisher authenticity.
6. **crates.io requires the ordered publish.** Because the workspace is not a
   single crate, publishing `pizen-lynx` requires the six internal crates to be
   published first; the hermetic registry test proves the graph installs without
   requiring credentials.
7. **macOS arm64 deployment target.** The cross build sets
   `MACOSX_DEPLOYMENT_TARGET=13.4` to match the upstream ONNX Runtime prebuilt.
   Older macOS versions are not supported by that artifact.
