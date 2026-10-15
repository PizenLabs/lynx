# Lynx Hardening Report

Companion to `production-audit.md` (findings) and `production-contract.md`
(guarantees). All numbers are measured on the development machine (Apple
silicon, release build) with the harness in `benchmarks/scripts/run_hardening.sh`.

---

## 1. Root cause of the memory/resource issue

There was not one leak. There were four independent, compounding unbounded
paths, three of them on the **indexing** side and one on the **query** side:

1. **Per-file lexical writer churn.** `TantivyStore::add_documents` created a
   fresh 15 MB `IndexWriter` and committed *per file*. Profiler evidence: the
   lexical stage was **24.996 s of 25.467 s (98%)** on a 200-file repo. Each
   commit rewrote segment metadata and spawned merge work; many transient
   segments were mmap'd (page cache counted as RSS).
2. **Whole-file module snippets.** Module pseudo-symbols retained and embedded
   the *entire file text*. Retained memory and embedding cost scaled with total
   repository bytes, and one long text per file inflated the entire per-file
   embedding batch (fastembed pads each batch to its longest member).
3. **Overlapping 25-line snippets.** Every symbol's excerpt was a 25-line window
   anchored at the first *substring* match of its name, so ~11 excerpts per file
   overlapped heavily — multiplying embedding cost and making reported ranges
   ambiguous.
4. **Eager full-candidate evidence materialization in `search`.** `search`
   collected *all* N semantic candidates, fused over the union, and constructed
   a full `Evidence` (cloning identity + the whole `Snapshot` + the whole
   snippet) for **every** candidate before `truncate(limit)`. Per-query memory
   was `O(N × snippet size)` and unbounded; repeated in a long-lived process it
   fragmented the allocator. There was **no cancellation or timeout anywhere**,
   so such a query ran to completion (or OOM) — matching "keeps running … RSS
   grows beyond 15 GB if the process is not stopped".

## 2. Exact lifecycle/ownership failure

`Engine` was the retention root for `symbols` (HashMap of snippets) and
`embeddings`, and `index_repository` amplified every file into
`O(file size)` retained text plus overlapping embedding texts. `search` then
allocated `O(N)` transient `Evidence` per call under a held lock with no
cancellation. The writer was owned per-call instead of per-pass, so writer
construction/teardown was on the hot path. In short: **resource lifetime was
per-file/per-candidate instead of per-pass/per-query.**

## 3. Fix applied

| Area | Change | File |
|---|---|---|
| Lexical writer | One shared `IndexWriter`, created lazily, committed once per pass (`commit()`); `search_snippets` commits pending docs first | `lynx-storage/src/tantivy_store.rs` |
| Embedding text | Compact bounded descriptor `"{fqdn} {Kind}\n{snippet[:512]}"` (128 for modules) instead of raw excerpts | `lynx-core/src/engine.rs` |
| Snippets | Non-overlapping, source-ordered excerpts anchored at whole-word occurrences; module snippet bounded to 80 lines | `lynx-core/src/engine.rs` |
| Query memory | Bounded top-k selection (`O(N log k)` time, `O(k)` memory) for semantic search and `similar`; candidate cap before materialization | `lynx-core/src/retrieval.rs`, `engine.rs` |
| Exact resolution | `O(1)` precomputed fqdn/name index with deterministic tie-break (was an `O(N)` scan) | `lynx-core/src/engine.rs` |
| Lifecycle bounds | `QueryOptions` + `CancelToken` + deadline + reports, threaded through `search_with`/`similar_with`/`context_with`/`trace_with` | `lynx-core/src/query.rs`, `engine.rs` |
| Determinism | Score→hash tie-breaks in fusion, ranking, and top-k | `lynx-core/src/retrieval.rs` |
| Idempotence | `INSERT OR IGNORE` for snapshots/symbols/relations so re-indexing is safe | `lynx-storage/src/graph_store.rs` |

## 4. Resource bounds introduced

- Candidate bound `clamp(limit*20, 100, 2000)`; caller override capped at
  100 000 (`CANDIDATE_HARD_MAX`).
- Semantic selection and `similar` are `O(k)` memory.
- Fused/materialized evidence is `O(candidates)`, never `O(symbols)`.
- Snippet ≤ 25 lines; module ≤ 80 lines; embedding text ≤ 512 chars (128 module).
- One lexical writer per pass; one ONNX model per process.
- Trace bounded by `depth` + visited set.

## 5. Cancellation behaviour

- `CancelToken::cancel()` from any thread stops a running query; observed at
  every semantic candidate (`take_while`) and every graph frontier step.
- `QueryOptions::with_timeout_ms` sets a wall-clock deadline.
- Aborted queries return `CoreError::Cancelled` / `CoreError::TimedOut` and the
  engine remains usable (`lifecycle::cancelled_query_releases_and_errors`).
- MCP `search`/`similar`/`context` accept `timeout_ms` (`search_timeout_is_enforced_per_call`).
- No background tasks/channels exist to orphan.

## 6. Retrieval failures discovered

Via the benchmark (`benchmarks/retrieval-corpus` + `retrieval-truth.json`,
10 queries across exact/definition/conceptual classes):

- **Exact symbol queries were ranking problems.** `search "Login"` returned the
  UI `Login`/test `TestLogin` before the auth `Login` because ranking — not
  deterministic resolution — decided. Violates "exact resolution must not
  become a ranking problem".
- **Test doubles leaked into results** (`auth::TestLogin` in the top 5 of an
  exact query) even though the test *path* was indexed.
- **Conceptual/flow queries remain weak** ("authentication flow" → recall 0.33).
- Module pseudo-symbols (whole-file, generic fqdn) polluted results.

## 7. Retrieval improvements (benchmark-justified)

- **Exact-symbol routing:** bare-symbol queries resolve deterministically via the
  `O(1)` index and are pinned first; a miss degrades to hybrid retrieval.
- **Rank-time noise penalty:** test/mock/fixture/generated paths ×0.5;
  vendor/third-party ×0.25. Removed `TestLogin` from the exact-`Login` top 5.

## 8. Benchmark methodology

`benchmarks/scripts/run_hardening.sh` runs, reproducibly:

1. indexing scaling (50/100/200/400 files × 10 symbols) → wall time + max RSS;
2. the retrieval benchmark with the hermetic fake provider (regression) and the
   real `bge-small-en-v1.5` embedder (quality): Precision@1, Recall@5, Hit@5, MRR;
3. a 300-query MCP RSS probe reporting the RSS-vs-query slope.

Ground truth lives **outside** the indexed corpus (`retrieval-truth.json`).

Latest run: `benchmarks/reports/hardening_20261016_024901.txt`
(indexing 50–400 files; real-embedding Precision@1 0.700 / Recall@5 0.883 /
Hit@5 1.000 / MRR 0.920; MCP 300-query slope 0.0003 MB/query).

## 9. Before / after measurements

| Metric | Before | After | Change |
|---|---|---|---|
| `lx index` 50 files × 10 syms | 28.1 s / 541 MB | 5.0 s / 295 MB | **5.6× / 1.83×** |
| `lx index` 100 files × 10 syms | 58.2 s / 552 MB | 9.2 s / 307 MB | **6.3× / 1.80×** |
| `lx index` 200 files × 10 syms | 111.2 s / 550 MB | 17.1 s / 313 MB | **6.5× / 1.76×** |
| `lx index` 400 files × 10 syms | 222.2 s / 572 MB | 33.7 s / 311 MB | **6.6× / 1.84×** |
| Index, per-file | 0.55 s | 0.084 s | **6.5× faster** |
| Lexical stage, 200 files (profile) | 24.996 s | 0.042 s | **≈595× faster** |
| MCP RSS over 300 queries | not isolated | **+0.1 MB** (0.0004 MB/query) | no accumulation |
| Retrieval Precision@1 (real) | 0.60 | **0.70** | +0.10 |
| Retrieval MRR (real) | 0.870 | **0.920** | +0.05 |
| Retrieval Recall@5 (real) | 0.883 | 0.883 | = |
| Retrieval Hit@5 (real) | 1.000 | 1.000 | = |
| Workspace tests | 51 | **77** | +26 |

Scaling remains linear in file count — but with a ~6.5× smaller constant and
~1.8× lower resident set. Decomposing the improvement on 200 files:

- storage writer fix (lexical churn): the stage profiler attributes 24.996 s of
  25.467 s to lexical writer create+commit before the fix, 0.042 s after;
- embedding descriptors: with the writer fixed but whole-file embedding texts,
  200 files cost 86.6 s; after bounding the per-symbol descriptors to 512/128
  chars, the same 200 files cost 17.1 s (fastembed pads each batch to its longest
  member, so one long text per file had inflated the whole batch);
- what remains is genuine ONNX inference, now the dominant per-file cost.

The MCP probe is the direct answer to the reported failure mode: a long-lived
process issuing hundreds of queries shows a flat RSS slope, so queries no longer
accumulate resources.

## 10. Regression tests

- `lynx-core/tests/lifecycle.rs`: repeated indexing idempotence; 100 stable
  searches; repeated session open/index/drop; empty repository → `NotIndexed`;
  malformed UTF-8 + binary + oversized files contained; bounded module/symbol
  snippets; vendor excluded + test demoted; cancellation/timeout + recovery.
- `lynx-core` unit tests: bounded top-k (order, k=0, ties, non-finite),
  deterministic fusion, deterministic duplicate-name resolution, pre-cancelled
  and expired queries, candidate bounds + report, cancellable `similar`,
  non-overlapping anchored snippets, whole-word matching.
- `lynx-mcp` tests: malformed-frame containment, per-call timeout, 49-call
  response stability, plus the existing envelope/contract tests.
- `lynx-storage`: lexical index/search round-trip through the shared writer.

## 11. Remaining risks

1. **Retrieval transports re-index on every invocation.** `lx search|resolve|
   relations|context` and MCP build an ephemeral index at start; the engine has
   no hydrate path, so a cold `lx search` pays a full indexing pass (now ~6×
   cheaper, but still a pass). `lx index` writes `.lynx` that nothing reads.
   This is the largest remaining product gap. Fix direction: persist
   symbols+embeddings (and hydrate), or make retrieval hydrate from the SQLite
   graph + a persisted vector store.
2. **Indexing retains `O(symbols)` in RAM** (snippets + vectors). Query memory is
   bounded, but a very large monorepo still needs an OS memory cap during
   indexing. Documented; subprocess isolation recommended for untrusted inputs.
3. **Semantic scan is `O(N·D)` per query** (memory is `O(k)`). A persistent/ANN
   vector index is a future improvement, not required for correctness.
4. **Conceptual/flow queries remain weaker** than exact/definition queries; no
   query expansion or intent-specific strategy yet.
5. **`resolve` returns a single winner per trailing name** (deterministic).
   Ambiguous duplicates are not surfaced as alternatives.
6. **Parser AST ranges are discarded**; snippet anchors are whole-word
   heuristics, not exact AST declarations. Correct ranges would further improve
   definition coordinates and de-overlap.

## 12. Production readiness assessment

**Not yet production-ready, but the production blocker is resolved.**

Ready:
- No known unbounded *per-query* memory path; query memory is `O(candidates)`.
- No orphaned query/task lifecycle; no per-query background tasks or channels.
- Cancellation and timeouts work and are observable.
- Repeated queries do not accumulate resources (measured).
- Repeated index access/opening is idempotent and does not accumulate.
- Exact symbol resolution is deterministic and separated from ranking.
- Ranking is explainable (`--explain`), benchmarked, and regression-tested.
- Malformed repositories/queries and empty repositories are contained.
- MCP lifecycle is stable under malformed frames and repeated calls.

Blocking production:
- **Persistent index reuse** (item 1 above). Until retrieval hydrates a persisted
  index, every cold query pays a full indexing pass, which is unbounded in the
  repository's size and is the next hardening target.
- **Hard memory ceilings for indexing** on very large/untrusted repositories
  (item 2): recommend subprocess isolation with an OS limit until an
  out-of-core index exists.

Recommended next slice: **persist and hydrate the index** (symbols, snippets,
embeddings, snapshot) so `lx search`/MCP become `O(query)` instead of
`O(repository)`, then re-run this harness to confirm cold-query latency and RSS.
