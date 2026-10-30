# Lynx — Production Readiness

Release contract for the final productization pass. This is the capability
matrix and the production gate. The current before/after numbers and their
method are in the
[2026-10 performance audit](../audit/lynx-performance-and-productization-report.md);
the earlier [`final-product-report.md`](final-product-report.md) remains the
historical productization report. Raw runs are under `benchmarks/reports/`.

Status vocabulary is exact and non-overlapping:

- **PASS** — meets its declared target with reproducible evidence.
- **CONDITIONAL** — works and is useful, but a declared, measured limit applies.
- **FAIL** — does not meet its declared target.
- **UNSUPPORTED** — explicitly out of scope; not claimed.

No status means "probably fine".

---

## 1. Release contract matrix

| Area | Status | Evidence |
|---|---|---|
| Exact resolution | **PASS** | `resolve` by fqdn or trailing name: P@1 1.000 (35 tasks); deterministic across reopen and re-index; ambiguity surfaced by `resolve_all`. |
| Lexical retrieval | **PASS** | BM25 over stored snippets with identifier word expansion: P@1 0.750, R@5 0.975, MRR 0.910 (20 tasks, real embeddings). |
| Semantic retrieval | **CONDITIONAL** | P@1 0.550, Hit@5 0.900, R@10 0.925, MRR 0.800 (20 tasks, real embeddings). Recall is strong; top-1 is limited by embedding representation, not by fusion (see the [performance audit](../audit/lynx-performance-and-productization-report.md) §C and §F). |
| Structural retrieval | **PASS** | Exact coordinate lookup: P@1 1.000 (15 tasks). |
| Relations | **PASS** | Intra-file call/containment graph: P@1 0.933, Recall 1.000 (15 tasks). Scope limited to intra-file edges. |
| Context | **CONDITIONAL** | Token-budgeted package: Hit@5 0.800, R@5 0.800, P@1 0.300. The relevant symbol is usually present; its ordering inherits the retrieval P@1. |
| Persistence | **PASS** | index → persist → exit → restart → hydrate → query, with no source read and no re-embed; `query_without_sources_returns_results` and `db_unmodified_by_query` hold. |
| Incremental indexing | **PASS** | Add/modify/delete/rename/dir-move/branch-like multi-file updates touch only changed files; on a 200-file workspace an unchanged re-index is ≈2 400× faster than a full build and re-embeds nothing. |
| Freshness | **PASS** | `Fresh/Stale/Missing/Incompatible/Corrupt/Unknown`; `lx status` reports it, MCP `index_status` exposes it on request; verified `fresh→stale→fresh`. |
| Cancellation | **PASS** | `CancelToken` observed per candidate; a cancelled query returns `Cancelled` and the engine stays usable. |
| Timeout | **PASS** | `QueryOptions::deadline` enforced at stages and per candidate; MCP `timeout_ms: 0` yields `isError`. |
| Resource bounds | **PASS** | Bounded candidates (`100–2000`, hard max 100 000), bounded snippets, bounded embedding batches, 8 MiB per-file guard, chunked SQL binds. |
| Low-memory | **CONDITIONAL** | Peak RSS is flat in repository size and dominated by the ONNX model *when semantic retrieval or embedding work runs* (≈600 MB loaded on the macOS arm64 reference environment). Structural commands, lexical search, MCP startup on an existing index, and a no-change index are model-free (≈12 MB measured). Validated on the reference machine, not on 4/8/16/32 GB hosts. |
| Large repository | **CONDITIONAL** | Scaling was measured to 400 files / 4 400 symbols (93.4 s full index, 1.07 GB peak RSS) in the [performance audit](../audit/lynx-performance-and-productization-report.md) §C. Beyond the measured envelope, the exact semantic scan is `O(N)` and behavior is not guaranteed. |
| MCP | **PASS** | 8 tools, 100% success (200 calls), p50 8.04 ms / p95 8.29 ms / p99 8.60 ms, startup RSS 11.1 MB, RSS drift +2.80 MB after warm-up; malformed frames, unknown tools, zero timeout, huge queries, and restart all contained. |
| CLI | **PASS** | `index`/`search`/`resolve`/`relations`/`context`/`status`/`mcp` with clear help, useful errors, stable exit codes, machine-friendly output. |
| Determinism | **PASS** | Total orders in resolution and fusion (lexical-rank tie-break; hash never decides ranking); stable across repeated calls, restart, hydrate, re-index. |
| Corruption recovery | **PASS** | Schema-version mismatch resets to a clean substrate (`Incompatible`); `PRAGMA quick_check` detects corruption (`Corrupt`); `rebuild_repository` / `lx index --force` is the recovery path. |
| Multi-language | **PASS** | Rust, Go, TypeScript, JavaScript, Python at `L3Structural`; Markdown/YAML/JSON/TOML/generic at `L1Parsed`. Relations are intra-file only. |
| Agent utility | **PASS** | All six realistic MCP workflows reached grounded coordinates; the first semantic workflow paid the one-time ≈690 ms model load, later semantic calls were ≈15–17 ms and structural calls sub-millisecond; the context workflow returned a bounded package. See the [performance audit](../audit/lynx-performance-and-productization-report.md) §C. |

---

## 2. Production gate

### Correctness
- ✓ deterministic (total orders; restart/reopen/re-index invariance tests)
- ✓ no silent data loss (identical bodies in distinct files get distinct identities)
- ✓ no stale data served as fresh (freshness is explicit; queries answer from the
  immutable snapshot they were indexed with, and staleness is reported)
- ✓ ambiguity handled explicitly (`resolve_all`, CLI alternatives, MCP `resolve_all`)
- ✓ persistence correct (hydrate-only query path; DB unmodified by queries)

### Retrieval
- ✓ exact, structural, and relational retrieval are reliable
- ✓ lexical retrieval is useful
- ~ semantic retrieval is useful (recall strong; top-1 limited)
- ~ context is useful by recall, ordering inherits retrieval P@1

### Runtime
- ✓ bounded query memory, ✓ cancellation, ✓ timeout
- ✓ restart recovery, ✓ failure containment

### Performance
- ✓ fast cold start (no model for structural/lexical-only commands)
- ✓ fast hydration, ✓ fast common queries, ✓ stable tail latency
- ✓ no repository re-scan on the query path

### Resource behavior
- ~ low-memory validated by measured RSS, not by constrained hosts
- ✓ repository scaling measured against a declared envelope
- ✓ oversize inputs bounded (per-file guard)

### Persistence
- ✓ persistent index, ✓ hydration, ✓ incremental update
- ✓ freshness detection, ✓ corruption/incompatibility detection and recovery

### Integration
- ✓ CLI, ✓ MCP, ✓ consumer-agnostic interface, ✓ documented contract

### Product quality
- ✓ clear README, ✓ clear limitations
- ✓ reproducible benchmarks (`benchmarks/scripts/run_final.sh`)
- ✓ permanent regression suite
- ✓ no PizenLabs dependency in the engine

---

## 3. Verification commands

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
benchmarks/scripts/run_final.sh
```

---

## 4. Declared limits (explicit)

1. **Semantic top-1** is representation-limited (P@1 0.550). Recall is strong
   (Hit@5 0.900, R@10 0.925). Improving it requires a better embedding
   representation, not fusion-weight tuning; measured fusion changes did not
   help.
2. **Context ordering** (P@1 0.300) inherits retrieval ordering; the right
   symbol is usually present (Hit@5 0.800).
3. **Relations are intra-file.** Cross-file calls are not linked.
4. **Very large repositories** are supported up to the measured envelope; the
   semantic scan is `O(N)` exact (no ANN), and beyond the envelope performance
   degrades rather than being guaranteed.
5. **Low-memory hosts** were validated by measuring RSS, not by running the
   product on 4/8/16/32 GB machines.
6. **`resolve` on a genuinely ambiguous name** is deterministic but not always
   the intended symbol; `resolve_all` is the correct surface for disambiguation.
