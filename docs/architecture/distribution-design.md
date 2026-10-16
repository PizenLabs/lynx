# Lynx Distribution Design

This document specifies how Lynx is released, installed, verified, and consumed.
It is the design that [`distribution-audit.md`](distribution-audit.md) called
for, and it is realized by `install.sh`, `.github/workflows/release.yml`,
`lx doctor`, `lx mcp --check`, and `tests/distribution/`.

The design obeys one rule above all others: **do not touch the engine**. Only
the distribution boundary is added.

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
| State | install/state directory is writable |
| Repository | a git work tree, or a populated directory |
| Index | presence, symbol count, compatibility, freshness (`Engine::freshness`) |
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
- **CI**: `ci.yml` runs the hermetic installer suite on Linux + macOS;
  `release.yml` runs the full clean-machine suite natively on every target.
