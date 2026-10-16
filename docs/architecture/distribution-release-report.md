# Lynx Distribution & Onboarding — Release Report

Final report for the distribution phase, covering both distribution surfaces:

- **Part I — Prebuilt releases + installer** (validated on macOS arm64).
- **Part II — crates.io public crate** (this phase).

Status vocabulary: **PASS**, **CONDITIONAL**, **FAIL**, **UNSUPPORTED**.

Development/validation host: macOS arm64, `rustc 1.98.1`. Everything not run on
that host is marked CONDITIONAL and is gated by native CI.

---

# Part I — Prebuilt releases and installer

## 1. Architecture audit (baseline)

The pre-implementation audit is
[`distribution-audit.md`](distribution-audit.md) (Part II). It found: no
prebuilt binaries, no release workflow, no checksums, no installer, no
`lx doctor`, no `lx mcp --check`, and no support matrix. The core was already
clean: no PizenLabs runtime coupling, no telemetry, no daemon. Status: **PASS**
(audit complete and acted upon).

## 2. Release artifact matrix

| Target | Runner | Artifact | Status |
|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | **PASS** |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | **CONDITIONAL** |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | **CONDITIONAL** |
| macOS x86_64 | — | — | **UNSUPPORTED** |
| Windows | — | — | **UNSUPPORTED** |

Each archive contains a single `lx` binary at the root; the release publishes
`SHA256SUMS`. See [`release-readiness.md`](release-readiness.md).

## 3. Installer design and security

`install.sh` — POSIX `sh`, single file, `curl` + `tar` + SHA-256 only. Flow:
detect → resolve tag → download → **verify checksum** → unpack → **atomic
rename** → verify `--version` → PATH guidance. Default directory
`$HOME/.local/bin`; `--system` is the only path to `/usr/local/bin`. Full design
in [`distribution-design.md`](distribution-design.md) §4.

| Property | Mechanism | Status |
|---|---|---|
| HTTPS enforced | base URL must be `https://`; curl `--proto '=https'` | **PASS** |
| HTTP fails | rejected unless loopback + explicit test opt-in | **PASS** |
| Integrity before install | SHA-256 vs. `SHA256SUMS` before unpacking | **PASS** |
| No arbitrary RCE | no `eval`, no sourcing; staged verify path | **PASS** |
| No shell-config edits | PATH printed, never written | **PASS** |
| No telemetry / no privilege escalation | none present; `sudo` only on `--system` | **PASS** |

## 4. Installation results (macOS arm64)

`tests/distribution/install_test.sh` — all checks **PASS**: fresh install,
repeat install (idempotent), upgrade, downgrade refusal (existing preserved),
checksum mismatch (existing preserved), missing artifact, unsupported platform,
atomic replacement, non-HTTPS rejection, download-only, `--help`.

## 5. Clean-machine results (macOS arm64)

`tests/distribution/clean_machine_test.sh` — all checks **PASS**: installed the
real binary through `install.sh` into a fresh prefix; `lx --version` correct
outside the checkout; arbitrary repository indexed and queried
(`search`/`resolve`/`context`); `lx doctor` NOT READY → READY across indexing;
independent MCP consumer completed
search/resolve/context/index_status with a clean shutdown; no Cargo/Node/Python/
PizenLabs requirement.

## 6. Performance

| Metric | Value |
|---|---|
| Release archive | ≈12.4 MB |
| Installed binary | ≈32.5 MB |
| `lx --version` | ≈7 ms |
| `lx doctor` | ≈37 ms |
| `lx mcp --check` | ≈31 ms |
| `lx mcp` startup (`initialize`) | ≈130 ms |

`--version`, `doctor`, `status`, `resolve`, and `relations` load no embedding
model. **PASS** against the "fast to start" target.

---

# Part II — crates.io public crate

## 7. Audit findings

The current-state audit is [`distribution-audit.md`](distribution-audit.md)
(Part I). Key findings:

- The published crates.io crates (`pizen-lynx*` **0.1.4**) were built from an
  **older, different codebase** than the current source: published
  `pizen-lynx-core 0.1.4` exposes `Lynx`/pipeline (async), not today's `Engine`;
  published `pizen-lynx 0.1.4`'s CLI is not the documented product.
- `cargo publish --dry-run -p pizen-lynx` **FAILED**: the registry dependency
  resolved to the incompatible 0.1.4 API. `cargo install pizen-lynx` installed
  the wrong product.
- The published `pizen-lynx` package contained no README and no LICENSE file.
- Version/dependency drift: workspace `0.1.4` (already on crates.io) with
  internal requirements pinned at `^0.1.1`.
- `pizen-lynx-common` was a dead published crate (removed from the workspace).

## 8. Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Public/private model | **Hybrid** | Cargo requires all internal deps on the registry, the workspace must not be collapsed, and the internal crates are already published. They are published but **non-contractual**; only `pizen-lynx` is public. |
| Package name | **keep `pizen-lynx`** | `lynx` and `lynx-cli` are taken by unrelated crates; `pizen-lynx` owns the published slots and matches the repository identity. |
| Version | **0.2.0** | The current source is a breaking rewrite of 0.1.4; 0.1.4 cannot be republished. |
| Publish automation | **manual, ordered** | The boundary must be confirmed; the pipeline must not publish internal crates merely for convenience. |

## 9. Packaging remediation

| Item | Before | After |
|---|---|---|
| Version | `0.1.4` (taken) | `0.2.0` |
| Internal dep requirements | `^0.1.1` | `0.2.0` (= workspace version) |
| README in package | absent | included (crate-local symlink) |
| LICENSE in package | absent | included (crate-local symlink) |
| Metadata | description only | `description`, `keywords`, `categories`, `documentation`, `homepage`, `license`, `repository` |
| Product identity | "CLI for the Lynx platform." | names the `lx` CLI and `lx mcp` server |

Status: **PASS** for all seven packages (`cargo package` succeeds with a local
`[patch]` override and produces complete archives).

## 10. Registry install validation

`tests/distribution/cargo_install_test.sh` (new, this phase):

1. `cargo package` every workspace crate.
2. Assemble a local `local-registry` from those archives plus the fetched
   third-party closure, and **replace crates.io** with it.
3. `cargo install --offline pizen-lynx --version 0.2.0` with a fresh
   `CARGO_HOME`.
4. Assert `lx --version` == `lx 0.2.0`, `lx doctor` runs, the binary is outside
   the checkout, and no development-only dependency is required.

This proves the published package graph installs from a registry without the
source checkout, without publishing credentials. Status: **PASS**.

## 11. Supported installation channels

| Channel | Command | Status |
|---|---|---|
| Installer | `curl … install.sh \| sh` | PASS (macOS arm64; CI-native elsewhere) |
| Rust-native | `cargo install pizen-lynx` | PASS (registry install validated; public 0.2.0 publish is maintainer-run) |
| Source | `git clone … && cargo build --release -p pizen-lynx` | PASS |

All channels converge on `lx` and `lx mcp`.

## 12. MCP distribution

One canonical consumer-agnostic server: `lx mcp`. Protocol `2024-11-05`, eight
tools. Consumer configuration is documentation, not code; `lx mcp setup` was
evaluated and **not** implemented. See
[`mcp-compatibility.md`](mcp-compatibility.md) and [`../mcp.md`](../mcp.md).

## 13. Known limitations

1. Only macOS arm64 was executed on the development host; the other prebuilt
   targets are CI-native and CONDITIONAL (fail-closed release). macOS Intel
   (x86_64) is unsupported: upstream ONNX Runtime ships no x86_64 macOS build, so
   the embedding engine cannot be compiled for it.
2. crates.io publication of 0.2.0 is **maintainer-run** (credentials); the
   hermetic test proves the graph installs, but the real publish must be done in
   dependency order.
3. Binaries are unsigned (no notarization/codesign); checksums cover transport
   integrity only.
4. First index requires network to fetch the embedding model (~once).
5. No Windows support.

## 14. Final release contract

| # | Item | Status |
|---|---|---|
| 1 | Public package clearly defined (`pizen-lynx`) | PASS |
| 2 | Internal crates not accidentally public (non-contractual; documented) | PASS |
| 3 | Cargo package builds from a clean environment | PASS |
| 4 | `cargo install` works (registry-validated) | PASS |
| 5 | `lx` works outside the checkout | PASS |
| 6 | Version is consistent (single source; tag guard) | PASS |
| 7 | Supported targets are actually tested | CONDITIONAL (CI-native) |
| 8 | Release archives deterministic; checksums verified | PASS |
| 9 | Installer works and is idempotent | PASS |
| 10 | Failed install preserves the previous version | PASS |
| 11 | `lx doctor` works | PASS |
| 12 | `lx mcp` works | PASS |
| 13 | MCP consumer validation passes | PASS (independent client) |
| 14 | No PizenLabs runtime dependency | PASS |
| 15 | No hidden development dependency | PASS |
| 16 | No telemetry / no daemon / no silent config mutation | PASS |
| 17 | Documentation complete; support envelope truthful | PASS |

## 15. Recommendation

Ship **v0.2.0**: tag `v0.2.0` (matching the workspace version), let
`release.yml` build and publish the prebuilt archives, then run the ordered
`cargo publish` for the seven crates. Before announcing, confirm every matrix
job is green; if `ubuntu-24.04-arm` is unavailable, publish the two available
targets and mark Linux arm64 CONDITIONAL rather than claiming it.

The distribution layer meets the target: one product, one binary (`lx`), one
canonical MCP surface (`lx mcp`), multiple installation channels, and a minimal
public surface — with no changes to the indexing engine, retrieval,
persistence, or MCP protocol.
