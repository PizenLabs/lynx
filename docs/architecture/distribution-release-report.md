# Lynx Distribution & Onboarding — Release Report

Final report for the distribution phase. It states what was built, what was
validated and where, and what remains conditional. Status vocabulary throughout:
**PASS**, **CONDITIONAL**, **FAIL**, **UNSUPPORTED**.

Development/validation host: macOS arm64, `rustc 1.98.1`. Everything not run on
that host is marked CONDITIONAL and is gated by native CI.

---

## 1. Architecture audit

The pre-implementation audit is [`distribution-audit.md`](distribution-audit.md).
Key findings it drove:

- No prebuilt binaries, no release workflow, no checksums, no installer — install
  required a Rust toolchain.
- Version/tag drift (`Cargo.toml` `0.1.4` vs. newest tag `v0.1.3`) with no
  artifacts tied to a version.
- No `lx doctor`, no `lx mcp --check`, no support matrix, no consumer guidance.
- The core was already clean: no PizenLabs runtime coupling, no telemetry, no
  daemon, `curl | sh` safe to add without touching the engine.

Status: **PASS** (audit complete and acted upon).

## 2. Release artifact matrix

| Target | Runner | Artifact | Status |
|---|---|---|---|
| macOS arm64 | `macos-14` | `lynx-vX.Y.Z-darwin-arm64.tar.gz` | **PASS** |
| macOS x86_64 | `macos-15-intel` | `lynx-vX.Y.Z-darwin-x86_64.tar.gz` | **CONDITIONAL** |
| Linux x86_64 | `ubuntu-24.04` | `lynx-vX.Y.Z-linux-x86_64.tar.gz` | **CONDITIONAL** |
| Linux arm64 | `ubuntu-24.04-arm` | `lynx-vX.Y.Z-linux-arm64.tar.gz` | **CONDITIONAL** |
| Windows | — | — | **UNSUPPORTED** |

Each archive contains a single `lx` binary at the root; the release publishes
`SHA256SUMS`. See [`release-readiness.md`](release-readiness.md) and
[`distribution-design.md`](distribution-design.md).

## 3. Installer design

`install.sh` — POSIX `sh`, single file, `curl` + `tar` + SHA-256 only. Flow:
detect → resolve tag → download → **verify checksum** → unpack → **atomic
rename** → verify `--version` → PATH guidance. Default directory
`$HOME/.local/bin`; `--system` is the only path to `/usr/local/bin`. Full design
in [`distribution-design.md`](distribution-design.md) §4.

Status: **PASS**.

## 4. Security model

| Property | Mechanism | Status |
|---|---|---|
| HTTPS enforced | base URL must be `https://`; curl `--proto '=https'` | **PASS** |
| HTTP fails | rejected unless loopback + explicit test opt-in | **PASS** |
| Integrity before install | SHA-256 vs. `SHA256SUMS` before unpacking | **PASS** |
| No arbitrary RCE | no `eval`, no sourcing; staged verify path | **PASS** |
| Correct quoting | all expansions quoted | **PASS** |
| No temp races | private `mktemp -d`; same-dir `mktemp` + rename | **PASS** |
| Stay in bounds | writes only to staging + install dir | **PASS** |
| No shell-config edits | PATH printed, never written | **PASS** |
| No telemetry / no exfiltration | none present | **PASS** |
| No privilege escalation | `sudo` only via explicit `--system` | **PASS** |

Publisher authenticity: unsigned; checksums cover transport integrity only
(a declared limitation, §12).

## 5. Installation results (macOS arm64)

`tests/distribution/install_test.sh` — all checks **PASS**:

| Case | Result |
|---|---|
| Fresh install | PASS — installs and runs the requested version |
| PATH guidance | PASS — printed when the dir is not on PATH |
| Repeat install (idempotent) | PASS |
| Upgrade | PASS |
| Downgrade refusal (existing preserved) | PASS |
| Checksum mismatch (existing preserved) | PASS |
| Missing artifact (nothing installed) | PASS |
| Unsupported platform (explicit error) | PASS |
| Atomic replacement (no partial files) | PASS |
| Non-HTTPS base rejected by default | PASS |
| Download-only (writes archive + sums, installs nothing) | PASS |
| `--help` (exit 0, prints usage) | PASS |

## 6. Platform results

| Platform | Binary runs | `--version` | `doctor` | `index`/query | MCP | Overall |
|---|---|---|---|---|---|---|
| macOS arm64 | PASS | PASS | PASS | PASS | PASS | **PASS** |
| macOS x86_64 | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | **CONDITIONAL** |
| Linux x86_64 | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | **CONDITIONAL** |
| Linux arm64 | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | CONDITIONAL | **CONDITIONAL** |
| Windows | UNSUPPORTED | — | — | — | — | **UNSUPPORTED** |

CONDITIONAL targets are built and natively validated by `release.yml`; they were
not executed on the development host.

## 7. MCP compatibility results

Details and configuration: [`mcp-compatibility.md`](mcp-compatibility.md) and
[`../mcp.md`](../mcp.md).

| Integration | Status |
|---|---|
| Independent stdio consumer (`mcp_client.py`) | **PASS** |
| Claude Desktop / Claude Code | CONDITIONAL (config verified; handshake not run here) |
| Cursor | CONDITIONAL |
| VS Code (Copilot) | CONDITIONAL (config verified) |
| OpenCode | CONDITIONAL (config verified) |
| Cline / Windsurf | CONDITIONAL |
| Zed / Gemini CLI | CONDITIONAL |

Recorded from the PASS run: `initialize` ≈130 ms, `tools/list` = exactly the
eight canonical tools, `search`/`resolve`/`context`/`index_status` returned
grounded results, clean shutdown on EOF.

## 8. Clean-machine results

`tests/distribution/clean_machine_test.sh` (macOS arm64) — all checks **PASS**:

- Installed the real release binary through `install.sh` into a fresh prefix.
- `lx --version` correct; binary outside the development checkout.
- Arbitrary (non-git) repository: `lx doctor` → NOT READY before index,
  `lx index` built `.lynx`, `lx status` reported fresh, `lx search`/`resolve`/
  `context` returned grounded results, `lx doctor` → READY after indexing.
- Independent MCP consumer completed search/resolve/context/index_status with a
  clean shutdown.
- No Cargo/Node/Python/PizenLabs requirement for the installed binary; no
  absolute development paths; state confined to `<repo>/.lynx` and
  `$HOME/.cache/lynx`.

## 9. Performance

| Metric | Value |
|---|---|
| Release archive | ≈12.4 MB |
| Installed binary | ≈32.5 MB |
| `lx --version` | ≈7 ms |
| `lx doctor` | ≈37 ms |
| `lx mcp --check` | ≈31 ms |
| `lx mcp` startup (`initialize`) | ≈130 ms |
| First query | ≈5 ms |
| Warm query | ≈4.3 ms |

`--version`, `doctor`, `status`, `resolve`, and `relations` load no embedding
model. **PASS** against the "fast to start" target.

## 10. Failure testing

| Failure | Behavior | Status |
|---|---|---|
| Network unavailable | download fails; previous binary intact | PASS |
| Release unavailable (404) | download fails clearly | PASS |
| Checksum mismatch | explicit error; previous binary intact | PASS |
| Unsupported platform | explicit error; nothing installed | PASS |
| Permission failure | explicit "not writable"; nothing installed | PASS |
| Missing artifact in archive | explicit error after verify | PASS |
| Interrupted download | partial file only in staging dir; cleaned | PASS |
| Missing PATH | install succeeds; guidance printed | PASS |
| MCP startup failure | local to the consumer; never mutates the repo | PASS (by design + `mcp --check`) |
| Stale index | reported as `stale`; queries answer from the indexed snapshot and staleness is surfaced | PASS (inherited) |

## 11. Configuration behavior

- No global config file; no product-specific repository marker; `.lynx` is
  Lynx-owned infrastructure only.
- The installer never edits shell startup files and never edits consumer
  configuration.
- `lx mcp setup` was evaluated and **not implemented** (fragile auto-detection
  vs. documented snippets). See design §8.
- Consumer differences live only in documentation.

**PASS.**

## 12. Known limitations

1. Only macOS arm64 was executed on the development host; other targets are
   CI-native and CONDITIONAL (fail-closed release).
2. `ubuntu-24.04-arm` availability depends on the repository plan.
3. Binaries are unsigned (no notarization/codesign); checksums are the integrity
   baseline. Publisher authenticity is not provided.
4. First index requires network to fetch the embedding model (~once).
5. No Windows support.

## 13. Acceptance gate (project §22)

| # | Item | Status |
|---|---|---|
| 1 | `curl` installer works | PASS (loopback-served release; real HTTPS path in CI) |
| 2 | Installed `lx` works outside a checkout | PASS |
| 3 | `lx doctor` works | PASS |
| 4 | Repository can be indexed | PASS |
| 5 | Repository can be queried | PASS |
| 6 | MCP starts from the installed binary | PASS |
| 7 | Independent consumer discovers tools | PASS |
| 8 | Independent consumer queries the repository | PASS |
| 9 | Installation is idempotent | PASS |
| 10 | Failed install leaves the previous install intact | PASS |
| 11 | Checksums are verified | PASS |
| 12 | Supported platform matrix is tested | CONDITIONAL (macOS arm64 PASS; others CI-native) |
| 13 | No hidden PizenLabs dependency | PASS |
| 14 | No telemetry | PASS |
| 15 | No background daemon | PASS |
| 16 | No silent configuration mutation | PASS |
| 17 | Documented support envelope is truthful | PASS |

## 14. Release recommendation

**Ship v0.1.4 (macOS arm64 validated; other matrix targets gated by native CI).**

Recommendation: proceed to tag `v0.1.4` (matching the workspace version). Before
announcing, confirm all four `release.yml` matrix jobs are green; if
`ubuntu-24.04-arm` is unavailable on the current plan, publish the three
available targets and mark Linux arm64 CONDITIONAL rather than claiming it.

The distribution layer meets the release target: one command to install, one
command to verify, one stable MCP server usable by many consumers — with no
changes to the indexing engine, retrieval, persistence, or MCP protocol.

**Hard stop.** No engine, retrieval, persistence, agentic, telemetry, daemon, or
speculative MCP changes are included or recommended.
