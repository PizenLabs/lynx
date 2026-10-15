# Lynx Production Contract

This document states the runtime invariants Lynx guarantees to embedders and
invokers, and the bounds it enforces. It is the contract a consumer (an agent
runtime, the MCP server, the CLI) may rely on. It is paired with
`production-audit.md` (what was wrong) and `hardening-report.md` (what changed).

## 1. Layer boundary

Lynx turns a repository + an intent into **repository evidence** (symbols,
coordinates, snippets, relations). It does not reason, plan, authorize, execute,
or mutate the repository. Nothing about querying mutates the workspace; only
`lx index` writes the `.lynx` artifact.

## 2. Query lifecycle

Every retrieval primitive has a bounded lifecycle:

```
start -> execute -> complete | fail | cancel -> release query-local resources
```

- There are **no background tasks, threads, or channels** spawned per query. A
  query is a synchronous call; on return (or error) all query-local buffers are
  dropped by the language runtime.
- Locks (`symbols`, `embeddings`, `resolve_index`, the writer, the SQLite
  connection) are always released on return, including on the error paths.
- A query that is cancelled or times out returns `Err` **and** the engine
  remains usable for subsequent queries (see `lifecycle::cancelled_query_releases_and_errors`).

## 3. Cancellation and timeouts

- `QueryOptions { query_id, deadline, candidate_limit, cancel }` is accepted by
  `search_with`, `similar_with`, `context_with`, and `trace_with`.
- `cancel` is a cheaply cloneable `CancelToken`; calling `cancel()` from any
  thread stops the running query.
- `deadline` (`with_timeout_ms`) bounds wall-clock time.
- Cancellation is observed at every semantic candidate and at every graph
  frontier step, so it propagates through the full chain. Cancelled/timed-out
  queries return `CoreError::Cancelled` / `CoreError::TimedOut`.
- The MCP `search`, `similar`, and `context` tools accept `timeout_ms` so a
  caller can bound a single call without killing the server.

## 4. Resource bounds

| Resource | Bound | Where |
|---|---|---|
| Per-channel candidates | `clamp(limit*20, 100, 2000)`; caller override capped at 100 000 (`CANDIDATE_HARD_MAX`) | `engine::candidate_limit` |
| Semantic selection memory | `O(k)` via bounded min-heap (was `O(symbols)`) | `retrieval::top_k_scored` |
| Fused/materialized evidence | `O(candidates)` (was `O(symbols)`) | `engine::search_with` |
| Symbol snippet | 25 lines | `MAX_SNIPPET_LINES` |
| Module snippet | 80 lines (was the whole file) | `MAX_MODULE_SNIPPET_LINES` |
| Embedding text | 512 chars (128 for modules) | `MAX_EMBED_CHARS` |
| Lexical writer | one shared `IndexWriter` per index pass (was one per file) | `TantivyStore` |
| Trace frontier | bounded by `depth`, visited set | `engine::trace_with` |

There is **no code path whose memory scales with repository size per query**.
Semantic scan time is `O(N·D)` but memory is `O(k)`; a persistent/ANN vector
index is a future improvement, not a correctness dependency.

## 5. Determinism

Given `(repository, index, query, configuration)` the logical result is stable:

- exact resolution (`resolve`, and `search` for bare-symbol queries) is `O(1)`
  through a precomputed index with a deterministic tie-break (lexicographically
  smallest fqdn);
- ranking ties break on content hash;
- candidate fusion order is score-descending, then hash-ascending;
- exact symbol resolution bypasses ranking entirely (Rule: deterministic before
  probabilistic) and is never overridden by semantic similarity.

## 6. Observability

- `SearchOutcome { evidence, report, contributions }`.
- `QueryReport { query_id, state, lexical_candidates, semantic_candidates,
  fused_candidates, results, stages[], total_micros }`.
- `RankContribution { lexical_rank, semantic_rank, rrf_score, definition_boost,
  final_score }` explains why each result ranked.
- CLI: `lx search "..." --explain` prints the report and per-result contributions.
  Production output is unchanged without `--explain`.
- Indexing reports files/symbols/relations/capability; no source contents are
  logged.

## 7. Failure containment

- Malformed/binary files are skipped during indexing (`read_to_string` failure).
- Unparseable files are skipped.
- Empty repositories yield `CoreError::NotIndexed` rather than a panic.
- Malformed queries do not panic; malformed MCP frames return a JSON-RPC parse
  error and the server continues.
- Oversized files/symbols produce bounded snippets, never unbounded buffers.

## 8. What is explicitly NOT guaranteed (yet)

- **In-process isolation is not claimed.** For adversarial or
  arbitrary-untrusted repositories under hard memory ceilings, run indexing in a
  subprocess with an OS memory limit. Query-side memory is bounded, but indexing
  retains `O(symbols)` state in RAM by design.
- **Persistent index reuse is not implemented.** Retrieval transports
  (`lx search|resolve|relations|context`, MCP) build an ephemeral index at start
  and index the workspace once; they do not hydrate from `.lynx`. `lx index`
  writes `.lynx` but no read path consumes it. This is the largest remaining
  product gap (see the hardening report).
- **Conceptual (flow) queries remain weak** relative to definition/exact
  queries; see the retrieval benchmark.

## 9. Reproducible verification

```
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
benchmarks/scripts/run_hardening.sh
```
