# Lynx Production Readiness

Companion to [`product-validation-audit.md`](product-validation-audit.md)
(findings) and [`product-validation-report.md`](product-validation-report.md)
(measured results). This document states the capability matrix, the production
gate, and exactly what remains open.

All numbers are from `benchmarks/scripts/run_validation.sh` unless labelled
otherwise. "Before" = commit `0064c37`; "after" = the remediation in this pass.

## 1. Capability matrix

| Capability | Current | Target | Evidence |
|---|---|---|---|
| Exact resolution | **PASS** | deterministic, ambiguity-aware | P@1 1.000 (35 tasks); `resolve_all` surfaces shared names; stable across reopen (`persistence.rs`) |
| Lexical search | **PASS** | strong keyword recall | P@1 0.700, R@5 0.925, MRR 0.852 |
| Semantic search | **PASS** | useful paraphrase recall | P@1 0.550, R@5 0.825, Hit@5 0.950, MRR 0.804 |
| Conceptual search | **NEEDS IMPROVEMENT** | flow queries resolve to the right symbols | semantic P@1 0.550; context P@1 0.100 |
| Structural discovery | **PASS** | exact coordinates | P@1 1.000 (15 tasks) |
| Relations | **PASS** | call/containment edges | P@1 0.933, Recall 1.000 (intra-file graph) |
| Context | **NEEDS IMPROVEMENT** | smallest useful package, right symbol first | Hit@5 0.800, R@5 0.750, but P@1 0.100 |
| Persistence | **PASS** | index→persist→restart→hydrate→query | `query_without_sources_returns_results=True`; `db_unmodified_by_query=True`; hydrate 91 ms |
| Incremental indexing | **NEEDS IMPROVEMENT** | file-level update | full reset+rebuild per pass (correct, not incremental); no staleness detection |
| Cancellation | **PASS** | aborts and recovers | `CancelToken` observed per candidate; `next query works` tested |
| Timeout | **PASS** | bounded wall clock | `timeout_ms` enforced; MCP `zero_timeout_isError=True` |
| Low-memory operation | **NEEDS IMPROVEMENT** | 4 GB safe, flat with repo size | indexing peak RSS flat 320→330 MB (8× symbols); no hard cap; ONNX ~250 MB floor |
| Large repositories | **NEEDS IMPROVEMENT** | 500 MB+ graceful | measured to 8.4 k symbols; semantic scan is O(N·D) |
| MCP | **PASS** | predictable, consumer-agnostic | 100% success, p50 3.9 ms, p99 4.5 ms, containment verified |
| Multi-language | **PASS** | Rust, Go, TS/JS, Python | all five adapters; structural relations are **intra-file only** |
| Determinism | **PASS** | stable across runs/restart | score→hash tie-breaks; reopen invariance tests |
| Explainability | **PASS** | diagnose ranking | `lx search --explain`; `resolve` lists alternatives |

## 2. Production gate

### Correctness
- ✓ deterministic (tests + reopen invariance)
- ✓ no stale index state (full reset before each pass; `reindex_reflects_added_deleted_and_renamed_files`)
- ✓ exact resolution correct (P@1 1.000)
- ✓ ambiguity surfaced (`resolve_all`, CLI alternatives) — **not hidden**

### Retrieval
- ✓ strong exact (1.000) and lexical (P@1 0.700 / R@5 0.925)
- ✓ useful semantic (P@1 0.550 / Hit@5 0.950)
- ✓ useful structural (1.000) and relation (P@1 0.933 / Recall 1.000)
- ~ context construction useful by recall (Hit@5 0.800) but poorly ordered (P@1 0.100)

### Runtime
- ✓ bounded memory per query (`O(candidates)`, streamed)
- ✓ cancellation, ✓ timeout
- ✓ restart recovery (hydrate)
- ✓ no known orphan lifecycle (no per-query tasks/channels)

### Resource behaviour
- ✓ acceptable 4 GB/8 GB behaviour for repositories up to ~10 k symbols
- ✓ indexing peak RSS no longer scales with repository size
- ~ graceful degradation on very large repos is **not yet demonstrated**
- ✓ no uncontrolled host-memory consumption on normal inputs (bounded candidates; malformed input contained)

### Lifecycle
- ✓ persistent index, ✓ hydration, ✓ corruption/failure recovery (schema-version reset)
- ~ incremental update is **not** implemented (full rebuild only)

### Consumer interface
- ✓ CLI, ✓ MCP (8 tools), ✓ consumer-agnostic (no PizenLabs coupling)
- ✓ documented protocol (this doc set + README + crate docs)

### Evidence
- ✓ reproducible corpus (`benchmarks/corpus`), ✓ grounded truth (`tasks/` + `truth/`),
- ✓ regression suite, ✓ before/after measurements

**Verdict:** the critical correctness, lifecycle, persistence, and cancellation
items pass. The two open non-critical items are **context ordering** and
**large-repository/incremental behaviour**. Lynx is a production candidate for
small and medium repositories (≤ ~50 MB / ~10 k symbols) with a stable workspace.
It is **not yet production-ready** for repositories that mutate continuously or
exceed the measured size, because there is no incremental update and no
demonstrated very-large-repository behaviour.

## 3. Remaining risks (explicit)

1. **No incremental indexing.** Every `lx index` rebuilds from scratch, and a
   hydrated index cannot tell that the workspace changed. Mitigation: consumers
   can compare `index_status().snapshot` (`commit_hash`, `is_dirty`,
   `content_hash`) and re-index. Next step: file-level diffing with targeted
   deletion of stale rows/vectors/documents.
2. **Semantic scan is O(N·D) per query** (memory is O(k)). At ~10 k symbols
   (≈15 MB of vectors) the scan is sub-millisecond; a 500 MB repository would
   make it seconds. Next step: a persisted, disk-backed vector index — only if
   measurement demands it.
3. **Context ordering.** The right symbol is usually in the package
   (Hit@5 0.800) but rarely first (P@1 0.100). This is a ranking problem inside
   the compiler, not an indexing problem.
4. **Indexing has no hard memory ceiling.** Peak RSS is now flat and dominated by
   the ONNX model, but an adversarial repository (e.g. one enormous file) can
   still spike. `index_repository` isolates per file; subprocess isolation with an
   OS limit remains the recommendation for untrusted input.
5. **Relations are intra-file.** Cross-file calls are not linked, which caps
   relation recall on real repositories.
6. **Staleness is not enforced.** A query on a stale index returns stale results
   (correctly, from the index) with no warning beyond `index_status`.
7. **`resolve` on a genuinely ambiguous name is deterministic but not always
   "right"** (e.g. `Login` → the lexicographically smallest production symbol).
   `resolve_all` is the correct surface for that case; consumers must use it when
   disambiguation matters.
8. **Very large repositories and cross-language monorepos are unmeasured.**

## 4. Error model

`CoreError` already distinguishes: `Storage`, `Parse`, `Embed`, `Io`, `Git`,
`NotIndexed`, `DimensionMismatch`, `Cancelled`, `TimedOut`. Mapped to the
required classes:

| Required class | Representation |
|---|---|
| NotIndexed | `CoreError::NotIndexed` (also MCP `index_status`) |
| IndexCorrupt | `CoreError::Storage(StorageError::Protocol/Sqlite)`; schema-version mismatch resets to empty |
| UnsupportedRepository | no supported files indexed → empty index → `NotIndexed` |
| ParseFailure | `CoreError::Parse`; per-file failures are skipped, not fatal |
| Cancelled | `CoreError::Cancelled` |
| TimedOut | `CoreError::TimedOut` |
| ResourceLimit | enforced by clamping candidate counts (documented bounds) rather than erroring — a deliberate choice favouring graceful degradation |
| InvalidQuery | rejected at the transport boundary (CLI mode/kind, MCP `-32602`); malformed *lexical* queries degrade to no lexical candidates rather than failing |
| AmbiguousResolution | surfaced by `resolve_all` (not an error) |
| InternalFailure | `CoreError::Storage/Embed/Io/Git`; MCP maps to `-32603` / `isError` |

## 5. Verification commands

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
benchmarks/scripts/run_validation.sh
```
