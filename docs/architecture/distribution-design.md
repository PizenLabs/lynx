# Lynx Distribution Design

This document specifies how Lynx is released, installed, verified, and consumed.
It is the design that [`distribution-audit.md`](distribution-audit.md) called
for, and it is realized by `install.sh`, `.github/workflows/release.yml`,
`lx doctor`, `lx mcp --check`, `tests/distribution/`, and the crates.io publish
graph.

The design obeys one rule above all others: **do not touch the engine**. Only
the distribution boundary is added. Part I (§1–§12) is the installer/prebuilt
release design; Part II (§13–§18) is the public-crate and crates.io design.

---

## 1. Goals and non-goals

**Goals**

1. `curl -fsSL <url> | sh` installs `lx` with no Rust/Node/Python/Docker runtime.
2. `lx --version` and `lx doctor` prove the installation quickly.
3. One canonical stdio MCP server (`lx mcp`) serves every consumer.
4. Releases are deterministic, inspectable, and checksum-verified.
5. Every failure is local, explainable, and leaves the previous install intact.

**Non-goals (hard stop)**

- No engine, retrieval, ranking, persistence, or schema changes.
- No new MCP tools and no consumer-specific code in core.
- No telemetry, no daemon, no auto-updater, no background service.
- No global cursor/agent configuration mutation. Consumer setup is documentation,
  not code (`lx mcp setup` was evaluated and deferred — see §8).
- No Windows support claim (§2).

---

## 2. Release artifact matrix

Artifacts are built on **native runners per target** — no cross-compilation, and
no support claimed without a native build of the real binary.

| Target | Runner | Artifact | Status |
|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | built + native-validated in release CI |
| macOS x86_64 | `macos-15-intel` | `lynx-vX.Y.Z-darwin-x86_64.tar.gz` | built + native-validated in release CI |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | built + native-validated in release CI |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | built + native-validated in release CI |
| Windows | — | — | **UNSUPPORTED** — no target built or claimed; the installer rejects it explicitly. |

**Archive contents.** Each archive contains a single file, `lx`, at the archive
root, mode `0755`. There are no directories, no README, no data files. The
embedding model is *not* bundled (it is downloaded once, lazily, into the user
cache; see §5).

**Integrity.** Every release publishes a `SHA256SUMS` manifest
(`sha256sum lynx-*.tar.gz`) alongside the archives. `install.sh` downloads and
verifies it before unpacking. Checksums are the appropriate baseline: the
repository has no existing signing infrastructure, and inventing one is out of
scope. The manifest is generated after all matrix jobs succeed, so a partial
release cannot be published.

**Build flags.** The shipped profile is already correct for distribution:
`opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = true`.

---

## 3. Versioning and tag policy

- Single source of truth: `[workspace.package] version` in `Cargo.toml`.
- A release is a tag `vX.Y.Z` **that matches** the workspace version; the
  `version-guard` job fails the release otherwise. This closes the audit's
  version/tag drift gap.
- `lx --version` prints `lx X.Y.Z` (stable, script-parseable).
- `install.sh` accepts `latest` (default) or an explicit tag, normalized so both
  `v1.2.3` and `1.2.3` work.
- Installation refuses to downgrade: if the installed version is newer than the
  requested tag, it stops with an explicit message unless `--force` is passed.

---

## 4. Installer design (`install.sh`)

POSIX `sh`. Small, single-file, dependency-free beyond `curl`, `tar`, and a
SHA-256 tool (`sha256sum` or `shasum`). No `eval`. All variables quoted.

### 4.1 Flow

```text
detect OS/arch
    -> resolve tag (latest via /releases/latest redirect, or explicit)
    -> download archive + SHA256SUMS to a private mktemp dir
    -> verify SHA-256 before anything is unpacked
    -> unpack into a temp dir
    -> write the binary to a same-directory temp file
    -> atomic rename onto <dir>/lx
    -> run <dir>/lx --version to confirm
    -> print install path and PATH guidance
```

The order is **download → verify → unpack → atomic rename**, never "download
directly over the installed binary". Any failure before the rename leaves the
existing `lx` untouched.

### 4.2 Platform detection

- OS: `uname -s` → `Darwin` → `darwin`, `Linux` → `linux`; anything else fails
  with an explicit "unsupported operating system" message.
- Arch: `uname -m` → `arm64`/`aarch64` → `arm64`, `x86_64`/`amd64` → `x86_64`;
  anything else fails explicitly.

### 4.3 Install directory

- Default: `$HOME/.local/bin` (user-owned, no sudo).
- `--dir DIR` / `LYNX_INSTALL_DIR` overrides.
- `--system` opts into `/usr/local/bin` **explicitly**. If that directory is not
  writable, the installer stops and tells the user to re-run with `sudo`; it
  never escalates privileges on its own.

### 4.4 Version selection

- `LYNX_VERSION` / `--version TAG`.
- `latest` resolves through the unauthenticated `/releases/latest` redirect — no
  API token, no JSON parsing, no rate limit.
- Downgrade refusal (§3).

### 4.5 Modes and flags

| Flag / env | Effect |
|---|---|
| `--version TAG`, `LYNX_VERSION` | select the release |
| `--dir DIR`, `LYNX_INSTALL_DIR` | choose the install directory |
| `--system` | install to `/usr/local/bin` (explicit system install) |
| `--download-only[=DIR]` | download + verify only; never installs |
| `--force` | reinstall even if the installed version is newer |
| `--help` | usage, exit 0 |
| `LYNX_REPO`, `LYNX_BASE_URL` | override the release source |
| `LYNX_DOWNLOAD_DIR` | target dir for `--download-only` |
| `LYNX_ALLOW_INSECURE` | loopback-http override for the test suite only |

### 4.6 PATH guidance

If the install directory is not on `$PATH`, the installer prints the exact
`export PATH=...` line. It **never** edits shell startup files.

---

## 5. State and configuration

| Artifact | Location | Owner |
|---|---|---|
| Repository index | `<repo>/.lynx/` | Lynx (per-repository, gitignored by convention) |
| Embedding-model cache | `$HOME/.cache/lynx/fastembed` | Lynx (per-user) |
| Installed binary | `$HOME/.local/bin/lx` | user |

Lynx requires **no** product-specific repository marker (`.izen`, `.ohgo`,
`.pizen`, or a mandatory `.lynx` file tracked in git). `.lynx` is Lynx-owned
infrastructure only and is excluded from indexing. There is no user-global
config file. The only environment variables the product reads are `HOME`,
`TMPDIR`, and `LYNX_INCLUDE_TESTS`.

---

## 6. `lx doctor`

A fast, model-free diagnostic (measured ≈37 ms). It never loads the embedding
model and never indexes. Checks:

| Check | Source |
|---|---|
| Binary | the process is running |
| Version | `CARGO_PKG_VERSION` |
| Platform | `OS` + arch, in release-matrix spelling (`macOS arm64`) |
| Install | `current_exe()` |
| Workspace | git toplevel of the CWD, else the CWD |
| State path | `<workspace>/.lynx` |
| Permissions | the state directory is writable |
| Repository | a git work tree, or a populated directory |
| Index | presence, symbol count, freshness (`Engine::freshness`) |
| Capability | index capability level (L0–L3) |
| MCP | in-process protocol smoke check |

`Status READY` requires a fresh index and a passing MCP check; otherwise
`Status NOT READY` with targeted next steps. Doctor exits non-zero only if the
diagnosis itself cannot run.

## 6a. `lx mcp --check`

An in-process smoke check that starts a session over an **ephemeral empty
workspace**, performs `initialize` → `tools/list` → `ping` → `tools/call
index_status`, and requires the advertised tools to equal the canonical eight.
It loads no model and indexes no repository, so it is safe to run anywhere
(≈31 ms).

---

## 7. MCP contract (unchanged)

One canonical server, consumer-agnostic:

```text
lx mcp                 # stdio JSON-RPC 2.0, workspace = git toplevel of CWD
lx mcp /path/to/repo   # explicit workspace (avoids ambiguous consumer cwd)
```

- Protocol version: `2024-11-05`.
- Tools (exactly eight): `search`, `resolve`, `resolve_all`, `inspect`,
  `relations`, `similar`, `context`, `index_status`.
- Hydrates `<workspace>/.lynx`; builds once at startup only if absent.
- Diagnostics to stderr; stdout carries protocol frames only.

The optional explicit `PATH` argument exists solely so a consumer can pin the
workspace when its launch directory is ambiguous. It adds no consumer-specific
behavior and no new tool.

## 8. Consumer adapters (documentation, not code)

One MCP implementation; many configuration snippets. Because each consumer
formats configuration differently, and because auto-editing those files is
fragile and violates "never silently modify user configuration", **`lx mcp setup`
is deliberately not implemented**. Instead:

- [`docs/mcp.md`](../mcp.md) and
  [`mcp-compatibility.md`](mcp-compatibility.md) give the exact minimal
  configuration for the highest-value consumers.
- Consumers invoke the installed binary (`command: lx`, `args: ["mcp"]`); no
  Lynx logic is embedded in any consumer.

If a consumer ever genuinely requires a config adapter, it will be a thin,
documented snippet — never a branch inside `lynx-core`.

---

## 9. Security model

| Requirement | Mechanism |
|---|---|
| HTTPS only | the base URL must be `https://`; curl is pinned with `--proto '=https' --proto-redir '=https'` |
| Fail on HTTP | non-HTTPS base is rejected unless `LYNX_ALLOW_INSECURE=1` **and** the host is loopback (test-only) |
| Fail on missing artifact | `curl -f` fails on 4xx/5xx; the installer aborts |
| Verify integrity before install | SHA-256 checked against `SHA256SUMS` before unpacking |
| No arbitrary RCE | the installer downloads, verifies, unpacks, and renames; it never sources remote content and uses no `eval` |
| Correct quoting | no unquoted expansions; no word-splitting hazards |
| No temp races | a private `mktemp -d` staging dir; the atomic rename uses `mktemp` in the target dir |
| Stay in bounds | writes only to the staging dir and the install directory |
| No shell-config edits | PATH is printed, never written to rc files |
| No data exfiltration | the product contains no telemetry and never transmits repository contents |
| No privilege escalation | `sudo`/`/usr/local/bin` only on explicit `--system` |

Bootstrap caveat, stated plainly: `curl | sh` means trusting the installer as
delivered. The installer is a single auditable file; `--download-only` and
`--help` allow inspection without execution.

---

## 10. Failure behavior

| Failure | Behavior |
|---|---|
| Fresh install | installs to the chosen dir; prints path + PATH guidance |
| Existing install, same version | idempotent no-op replace; same result |
| Existing install, older version | upgrades |
| Existing install, newer version | refuses (explicit message); `--force` overrides |
| Unsupported OS/arch | explicit error, non-zero exit, nothing installed |
| Network unavailable | download fails, existing binary untouched |
| Release unavailable (404) | download fails, existing binary untouched |
| Checksum mismatch | explicit mismatch error, existing binary untouched |
| Corrupt archive | `tar` fails after verification; nothing installed |
| Interrupted download | partial file lives only in the staging dir; cleaned on exit |
| Permission failure | explicit "not writable" error; nothing installed |
| Missing PATH | install succeeds; guidance printed |
| Temp-dir failure | explicit error; nothing installed |

No failure path can produce a partially written `lx`: the target is replaced by a
single atomic rename, and only after verification.

---

## 11. Performance targets (measured, macOS arm64)

| Metric | Value |
|---|---|
| Release archive (`.tar.gz`) | ≈12.4 MB |
| Installed binary | ≈32.5 MB |
| `lx --version` | ≈7 ms |
| `lx doctor` | ≈37 ms |
| `lx mcp --check` | ≈31 ms |
| `lx mcp` startup (`initialize`) | ≈130 ms |
| First query in an MCP session | ≈5 ms |
| Warm query in an MCP session | ≈4.3 ms |

Structural commands (`--version`, `doctor`, `status`, `resolve`, `relations`)
load no embedding model.

---

## 12. Testing strategy

- **Installer** (`tests/distribution/install_test.sh`): fresh, repeat, upgrade,
  downgrade refusal, checksum failure, missing artifact, unsupported platform,
  previous-install preservation, atomic replacement, PATH guidance,
  download-only, HTTP rejection, `--help`. Hermetic (synthetic releases over
  loopback).
- **CLI** (`crates/lynx-cli/tests/cli.rs`): `--version`, `--help`, `doctor`,
  `mcp --check`, `status`-without-index.
- **MCP** (`crates/lynx-mcp/src/lib.rs`): existing eight-tool suite plus
  `smoke_check`.
- **Distribution / clean machine**
  (`tests/distribution/clean_machine_test.sh` + `mcp_client.py`): install the
  real binary, index an arbitrary repository, run
  status/search/resolve/context, run `doctor`, and drive the installed server
  with an independent MCP consumer.
- **crates.io / registry install** (`tests/distribution/cargo_install_test.sh` +
  `make_registry.py`): `cargo package` every crate, assemble a `local-registry`
  from the archives plus the fetched closure, replace crates.io with it, and
  `cargo install --offline pizen-lynx`; then verify `lx --version` and
  `lx doctor`. No publishing credentials.
- **CI**: `ci.yml` runs the hermetic installer suite on Linux + macOS and the
  registry-install test on Linux; `release.yml` runs the full clean-machine
  suite natively on every target.

---

# Part II — Public crate, workspace boundary & crates.io

## 13. Public surface and stability

The product is **Lynx**, and its public conceptual surface is exactly two
things:

```text
Lynx
├── CLI   (lx)
└── MCP   (lx mcp)
```

The implementation crates (core, storage, parser, embed, protocol, mcp) are
**not** part of that surface. This is the governing rule:

> **Modularity inside the workspace is not the same thing as public API.**

A crate does not become public because it is technically publishable. Promoting
an internal crate to a public API means committing to semver discipline,
documentation, a changelog, compatibility expectations, dependency stability,
API review, a migration policy, security maintenance, and user support.
Internal modularity must not automatically become external API surface.

### 13.1 Model A vs Model B — and the resolution

- **Model A — everything public.** Publish every crate as a supported API.
  Rejected: it commits Lynx to maintaining six implementation APIs it has no
  evidence anyone needs, and freezes internal refactoring.
- **Model B — public product, private implementation.** Publish only the
  product; keep the rest private. Preferred in principle, but **mechanically
  impossible here**: Cargo requires every normal/build dependency of a
  published crate to resolve from the registry, the workspace must not be
  collapsed (hard constraint), and the internal crates are already published at
  0.1.4 and cannot be removed from crates.io.

**Resolution — the hybrid.** The internal crates remain *published* (a Cargo
necessity) but are *not a public API*: they are documented as internal,
non-contractual implementation, and only `pizen-lynx` carries a stability
promise. Registry presence and API commitment are separate concerns; this design
separates them explicitly.

## 14. The published crate graph

Publication order is forced by the internal dependency graph (§I.2):

```text
1. pizen-lynx-protocol
2. pizen-lynx-parser
3. pizen-lynx-embed
4. pizen-lynx-storage
5. pizen-lynx-core
6. pizen-lynx-mcp
7. pizen-lynx          <- the product
```

Rules:

- **No published crate may depend on an unpublished local-only path crate.**
  Every internal dependency carries a `version` equal to the release version in
  `[workspace.dependencies]`, so the normalized manifest points at registry
  versions.
- **Version requirements track the release.** A lagging requirement (for
  example `^0.1.1` against a `0.1.4` workspace) is a release defect: it makes the
  published graph ambiguous and lets an incompatible older crate satisfy the
  requirement.
- **Internal crates are published only to satisfy Cargo.** No crate is published
  "because the pipeline is convenient". If the package boundaries ever change,
  revisit whether any published internal crate can be dropped.
- The product binary is `lx`. `pizen-lynx-mcp` also builds a binary, but it is an
  **auxiliary, undeclared entrypoint**; `lx mcp` is the single canonical MCP
  server (§7, §17).

## 15. Versioning authority

One authoritative version: `[workspace.package] version` in the root
`Cargo.toml`. Nothing may drift from it:

| Surface | Source |
|---|---|
| Cargo package versions | `version.workspace = true` in every crate |
| Internal dependency requirements | `[workspace.dependencies]` `version` |
| `lx --version` | `CARGO_PKG_VERSION` via clap |
| MCP `serverInfo.version` | `env!("CARGO_PKG_VERSION")` |
| Release tag / artifacts / checksums | tag `vX.Y.Z`; guarded by `release.yml` |
| Scripts (`install.sh`, distribution tests) | read the workspace version |

The release `version-guard` job fails any tag that does not equal the workspace
version, so drift cannot be released.

**Why 0.2.0.** The current source is a breaking rewrite of the previously
published 0.1.4 (the library API changed wholesale, and the CLI/MCP surface was
rebuilt). 0.1.4 already exists on crates.io and cannot be republished, and 0.x
patch releases imply compatibility. Under semver, the honest next release of
this source is **0.2.0**.

## 16. crates.io channel (`cargo install pizen-lynx`)

The supported installation channels converge on one product:

| Channel | Command | Audience |
|---|---|---|
| Installer (fastest) | `curl -fsSL …/install.sh \| sh` | Everyone |
| Rust-native | `cargo install pizen-lynx` | Rust developers |
| Source | `git clone … && cargo build --release -p pizen-lynx` | Contributors |

All three produce the same `lx` binary and the same `lx mcp` server. There are
no channel-specific binaries.

Publishing is **manual and ordered** (never automated until the boundary is
confirmed and the ordered publish is verified):

```bash
for p in pizen-lynx-protocol pizen-lynx-parser pizen-lynx-embed \
         pizen-lynx-storage pizen-lynx-core pizen-lynx-mcp pizen-lynx; do
    cargo publish -p "$p"          # or: cargo publish --dry-run -p "$p" first
done
```

A crate cannot be packaged/published until its internal dependencies already
exist on the registry; this is a Cargo invariant, not a policy choice. The
hermetic test in §18 exercises exactly this graph without requiring credentials.

## 17. Packaging rules

Every published package must be self-describing and self-contained:

- **README and LICENSE are included.** Each crate has `README.md` and `LICENSE`
  symlinks to the repository root, which Cargo dereferences into the archive.
- **Metadata is complete.** `description`, `keywords`, `categories`,
  `documentation`, `homepage`, `license`, and `repository` are set (shared fields
  via `[workspace.package]`).
- **Product description.** `pizen-lynx`'s description names the product and both
  surfaces (`lx`, `lx mcp`); internal crates are marked "internal … not a public
  API".
- **No absolute paths, no local-only files, no dev-only dependency required at
  install time.**
- The `pizen-lynx-mcp` binary is not advertised as a product surface.

## 18. CI and publish gate

- **`cargo_install_test.sh`** (`tests/distribution/`) is the crates.io analogue
  of the clean-machine test: it `cargo package`s every crate, assembles a local
  **sparse** registry from the archives, and runs `cargo install --registry` with
  a fresh `CARGO_HOME`, then runs `lx --version` and `lx doctor` from the
  installed binary outside the checkout. It requires no publishing credentials.
- The release workflow publishes **prebuilt archives** (`release.yml`) as the
  primary channel. crates.io publication is a separate, gated, manual step; the
  release job must not publish crates automatically.
- The release gate ([`release-readiness.md`](release-readiness.md)) must include:
  the ordered `cargo publish` for all seven crates, `cargo install pizen-lynx`
  succeeding from a clean environment, and `lx --version` matching the tag.
