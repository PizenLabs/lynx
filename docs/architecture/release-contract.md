# Lynx Release Contract

This is the normative release/distribution contract for Lynx. It is realized by
[`install.sh`](../../install.sh), [`.github/workflows/release.yml`](../../.github/workflows/release.yml),
[`scripts/release/`](../../scripts/release), and [`tests/distribution/`](../../tests/distribution).

The product identity is fixed:

```text
Product:        Lynx
Cargo package:  pizen-lynx     (the only stable public package)
Binary:         lx             (the only product binary)
MCP entrypoint: lx mcp         (the only MCP product surface)
```

---

## 1. Release state machine

A git tag is the single source of truth for a binary release. The classification
rule lives in exactly one place — `scripts/release/classify_tag.sh` — which the
workflow invokes; the distribution tests exercise the same script, so the
workflow decision and the tests cannot drift.

```text
TAG
 │
 ├── invalid ───────────────→ FAIL (no build, no release)
 │
 ├── prerelease ────────────→ VALIDATE ONLY → STOP
 │                             (build + smoke test; no artifacts, no Release)
 │
 └── stable ────────────────→ BUILD → VERIFY → RELEASE
```

SemVer, not substring matching:

| Tag | Kind | Effect |
|---|---|---|
| `v1.0.0`, `v1.0.1`, `v2.10.3` | **stable** | Production binaries + `SHA256SUMS` + GitHub Release |
| `v1.0.1-rc.1`, `v1.0.1-alpha.1`, `v1.0.1-beta.1`, `v2.0.0-dev.1` | **prerelease** | Validation only; **never** a production release |
| `v1`, `v1.0`, `v1.0.0foo`, `release-v1.0.0` | **invalid** | Fail closed |

There is **no path** from a prerelease tag into the stable binary publishing
path: the `build` and `release` jobs are gated on
`needs.classify.outputs.kind == 'stable'`, and the prerelease-only job neither
uploads artifacts nor creates a release. See `.github/workflows/release.yml`.

---

## 2. Versioning authority

One authoritative version: `[workspace.package] version` in the root
`Cargo.toml`. Nothing may drift from it.

For a stable tag `vX.Y.Z`, the pipeline proves:

```text
Git ref  ==  Git tag  ==  Cargo package version  ==  lx --version
```

For a prerelease tag `vX.Y.Z-pre`, the tag and package version must still agree
(`v1.0.1-rc.1` ⇔ package `1.0.1-rc.1`). The version-guard reads the **resolved**
Cargo metadata (not a textual grep), via `scripts/release/check_versions.sh`;
mismatches fail the pipeline closed. `check_versions.sh --binary <path>` also
proves the built binary reports the same version.

---

## 3. Release targets

Artifacts are built on **native runners per target**; there is no
cross-compilation and no support claimed without a native build.

| Target | Runner | Artifact | Policy |
|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | REQUIRED |
| macOS x86_64 | `macos-15-intel` | `lynx-vX.Y.Z-darwin-x86_64.tar.gz` | REQUIRED |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | REQUIRED |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | REQUIRED (CONDITIONAL runner availability) |
| Windows | — | — | **UNSUPPORTED** |

A declared target that cannot be built **fails the release** (fail closed): if
the `ubuntu-24.04-arm` runner is unavailable on the repository plan, the `build`
job fails and no release is published. No incomplete artifact set is ever
uploaded.

Each archive contains exactly one file — `lx`, at the archive root, mode `0755`
— and the workflow verifies this before publishing. The `SHA256SUMS` manifest is
generated from the **final** downloaded artifacts, and the release fails closed
if any archive is missing from it.

---

## 4. Installation channels

All channels converge on one product:

```text
curl ... install.sh | sh
cargo install pizen-lynx
git clone ... && cargo build --release -p pizen-lynx
        │
        ▼
       lx
     ┌──┴──┐
    CLI   MCP  (lx mcp)
```

### 4.1 Installer (`install.sh`)

POSIX `sh`, single file, `curl` + `tar` + SHA-256 only. Detect → resolve tag →
download → verify checksum → unpack → atomic rename → verify → PATH guidance.

- Defaults to a user-owned directory (`$HOME/.local/bin`); no `sudo`.
- Idempotent; a failed install leaves the existing `lx` untouched.
- No shell-profile mutation, no telemetry, no daemon, no background process, no
  auto-update.
- **Consumes only stable releases.** `latest` resolves through GitHub's
  `/releases/latest`, which excludes prereleases; an explicitly requested
  prerelease tag (`-rc`, `-alpha`, `-beta`, `-dev`, …) is rejected unless the
  testing-only override `LYNX_ALLOW_PRERELEASE=1` is set.

### 4.2 Cargo (`cargo install pizen-lynx`)

Installs the single product binary `lx`. The `pizen-lynx` package declares no
other binary. `tests/distribution/cargo_install_test.sh` proves this hermetically
from a locally assembled registry, with a fresh `CARGO_HOME`, outside the source
checkout.

### 4.3 Public surface vs published crates

The workspace is internally modular and is **not** collapsed. Cargo requires the
internal crates that `pizen-lynx` depends on to be registry-resolvable, so they
are published — under the rule:

> **Published ≠ public API.**

Only `pizen-lynx` is a stable, versioned public package. The internal crates
(`pizen-lynx-protocol`, `-parser`, `-embed`, `-storage`, `-core`, `-mcp`) are
implementation details: non-contractual, not guaranteed stable, and not a
user-facing API.

---

## 5. crates.io publication policy

crates.io publication is **deliberate and maintainer-controlled**. Automatic
GitHub Releases do not imply automatic crates.io publication.

- Publication order is forced by the dependency graph:
  `protocol → parser → embed → storage → core → mcp → pizen-lynx`.
- `.github/workflows/publish-crates.yml` is **manual-only** (`workflow_dispatch`)
  and requires an explicit confirmation input; it is never tag-triggered.
- Before a real publish, `cargo publish --dry-run` must succeed for every
  package that will actually be published.
- A published crate is never republished merely because it exists historically.
  `pizen-lynx-common` is a dead crate (removed from the workspace) and is **not**
  published or depended upon.

Real publication needs maintainer credentials (`CARGO_REGISTRY_TOKEN`) and a
confirmed public/private boundary; the hermetic registry test proves the graph
installs without them but is not a substitute for the real publish.

---

## 6. Reproducibility

For the same source commit and target, the release is deterministic at the
artifact-name and checksum level: the version comes from the Git tag / Cargo
metadata, artifact names are fixed, and checksums are computed over the final
archives only. The build does not embed timestamps or random data of its own.
This is **not** a claim of byte-for-byte reproducibility: the Rust toolchain and
build environment are not pinned to that degree (for example, the `stable`
toolchain channel and runner images move).

---

## 7. Verification

The contract is enforced by automated tests, not by convention:

- Release decision: `tests/distribution/release_test.sh` (classification table,
  fail-closed invalid tags, version consistency, workflow wiring, single-binary
  hygiene).
- Installer: `tests/distribution/install_test.sh` (fresh/repeat/upgrade,
  downgrade refusal, checksum failure, missing artifact, unsupported platform,
  atomicity, prerelease refusal).
- Clean machine: `tests/distribution/clean_machine_test.sh` (install the real
  binary, index/search/resolve/context, `doctor`, independent MCP consumer).
- Registry: `tests/distribution/cargo_install_test.sh` (package every crate,
  local-registry `cargo install pizen-lynx`, single `lx` binary, `doctor`,
  `mcp --check`).
