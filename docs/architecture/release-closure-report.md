# Lynx Release & Distribution Closure Report

Final distribution/release-engineering closure. The normative contract is
[`release-contract.md`](release-contract.md); this report records what existed
before, what changed, and the verification results.

Development/validation host: macOS arm64. Status vocabulary: **PASS**,
**CONDITIONAL**, **UNSUPPORTED**.

---

## 1. Audit — what existed before

### A. Release workflow (before)

```text
tag v*  (any tag starting with "v", including prereleases)
  │
  ▼  release.yml
  verify   : fmt / check / clippy / test
  version-guard : sed-grep Cargo.toml version == tag, else FAIL
  build    : native build per target + package + smoke + clean-machine
  release  : SHA256SUMS + `gh release create` (always, no prerelease check)
  │
  ▼
install.sh resolves latest or an explicit tag
```

`release.yml` had no tag classification. A prerelease tag whose version matched
the workspace version would have been published as a normal production GitHub
Release. The version guard used a textual `sed` grep of `Cargo.toml`, and the
built binary's reported version was never asserted against the tag.

### B. Package graph (before)

```text
pizen-lynx (bin: lx)
  -> pizen-lynx-core, pizen-lynx-embed, pizen-lynx-mcp, pizen-lynx-protocol
pizen-lynx-core
  -> pizen-lynx-embed, pizen-lynx-parser, pizen-lynx-protocol, pizen-lynx-storage
pizen-lynx-mcp (bin: pizen-lynx-mcp) -> core, embed, protocol
pizen-lynx-storage -> protocol
pizen-lynx-parser  -> protocol
pizen-lynx-embed / pizen-lynx-protocol -> (none)
```

`pizen-lynx-common` was already dead (removed from the workspace).

### C. Binary surfaces (before)

| Binary | Owning crate | Classification |
|---|---|---|
| `lx` | `pizen-lynx` | **PUBLIC** (the product) |
| `pizen-lynx-mcp` | `pizen-lynx-mcp` | **INTERNAL / ambiguous** — a second packaging of the MCP server |

### D. Installer source (before)

`install.sh` resolved the tag from `LYNX_VERSION`/`--version` or GitHub's
`/releases/latest` redirect, then downloaded
`https://github.com/${LYNX_REPO}/releases/download/${TAG}/${ASSET}` and
`…/${TAG}/SHA256SUMS`. It accepted **any** tag, including prereleases.

### E. Prerelease behavior (before)

- `v1.0.1`: if it matched the workspace version → production release. Otherwise
  the `version-guard` `sed` grep failed → release failed.
- `v1.0.1-rc.1`: same guard; if the workspace version were `1.0.1-rc.1` the
  prerelease would have been published as a **production** GitHub Release. There
  was no classification and no prerelease gate.

### F. Gaps (before)

**P0 — release correctness**
1. No SemVer tag classification; prereleases could reach the production path.
2. Malformed tags relied on a version mismatch, not an explicit rule.
3. Version guard used a textual grep, not resolved Cargo metadata.
4. The built binary's version was never asserted against the tag.

**P1 — distribution robustness**
5. Installer accepted prerelease tags.
6. Auxiliary `pizen-lynx-mcp` binary created an ambiguous second surface.
7. Target policy (REQUIRED/CONDITIONAL/UNSUPPORTED) was implicit.
8. No automated release-classification/version-guard tests.

**P2 — documentation**
9. No single normative release contract; prereleases undocumented.

---

## 2. Changes

| Area | File(s) | Change |
|---|---|---|
| Classification | `scripts/release/classify_tag.sh` (new) | Strict SemVer tag classification (`stable`/`prerelease`/`invalid`); the one source of truth. |
| Version guard | `scripts/release/check_versions.sh` (new) | tag ↔ resolved Cargo package version ↔ `lx --version`, fail closed. |
| Release workflow | `.github/workflows/release.yml` | `classify` job; `build`/`release` gated on `kind == stable`; `version-guard` uses Cargo metadata; binary-version + archive-content checks; `prerelease-validate` (no artifacts, no release); explicit target policy. |
| Installer | `install.sh` | Stable-only guard; refuses prerelease tags unless `LYNX_ALLOW_PRERELEASE=1`. |
| Product surface | `crates/lynx-mcp/src/main.rs` (removed), `crates/lynx-mcp/Cargo.toml` | The `pizen-lynx-mcp` crate is library-only; the workspace produces one binary, `lx`. |
| Release tests | `tests/distribution/release_test.sh` (new) | Classification table, version consistency/mismatch, single-binary hygiene, workflow-wiring assertions, unsupported target, workflow dry run. |
| Installer tests | `tests/distribution/install_test.sh` | Prerelease refusal + explicit override. |
| Registry tests | `tests/distribution/cargo_install_test.sh` | Single-binary assertion + `lx mcp --check`. |
| CI | `.github/workflows/ci.yml` | Runs `release_test.sh`. |
| Publish policy | `.github/workflows/publish-crates.yml` | Documents dry-run policy; no dead crates. |
| Docs | `docs/architecture/release-contract.md` (new), `release-readiness.md`, `distribution-design.md`, `distribution-audit.md`, `README.md`, `docs/cli.md` | Release state machine, target policy, prerelease semantics, published≠public. |

---

## 3. Release state machine

```text
TAG
 │
 ├── invalid ───────────────→ FAIL
 │
 ├── prerelease ────────────→ VALIDATE ONLY → STOP
 │
 └── stable ────────────────→ BUILD → VERIFY → RELEASE
```

---

## 4. Distribution architecture

```text
curl | sh
cargo install pizen-lynx
source build
        │
        ▼
       lx
     ┌──┴──┐
    CLI   MCP
          │
        lx mcp
```

---

## 5. Verification

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --workspace` | PASS |
| `cargo build --release --workspace` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS |
| `tests/distribution/release_test.sh` | PASS |
| `tests/distribution/install_test.sh` | PASS |
| `tests/distribution/clean_machine_test.sh` | PASS (macOS arm64) |
| `tests/distribution/cargo_install_test.sh` | PASS |
| Stable-tag dry run (`v1.0.1`) | build → release |
| Prerelease-tag dry runs (`v1.0.1-rc.1`, `-alpha.1`, `-beta.1`) | validation only, no release |

---

## 6. Remaining limitations

1. Only macOS arm64 was executed on this host; the other prebuilt targets are
   CI-native and CONDITIONAL.
2. `ubuntu-24.04-arm` runner availability is CONDITIONAL on the repository plan;
   its absence fails the release closed rather than publishing a partial set.
3. Real crates.io publication of `0.2.0` requires maintainer credentials
   (`CARGO_REGISTRY_TOKEN`) and the confirmed ordered publish; the hermetic
   registry test proves the graph installs without them.
4. Binaries are unsigned (no notarization), so checksums cover transport
   integrity, not publisher authenticity.
5. The first `lx index` requires network access to fetch the embedding model.
6. No Windows support.
7. Reproducibility is deterministic at artifact-name/checksum level, not
   byte-for-byte (the toolchain channel and runner images are not pinned).

## 7. Verdict

**RELEASE READY** for the distribution/release contract: a stable tag produces
the correct release, a prerelease tag cannot become one, invalid tags fail
closed, and every install path converges on `lx` / `lx mcp`.
