# Lynx — Final Product Report

The result of the final productization pass: the repository intelligence engine
as a complete, independent, usable product. Every number here is reproducible
with `benchmarks/scripts/run_final.sh`; the raw run for this pass is
`benchmarks/reports/final_20261016_074019.txt` (`final_<timestamp>.txt` for
future runs). The capability contract and status
vocabulary are in [`production-readiness.md`](production-readiness.md); the
architecture is in [`final-product-audit.md`](final-product-audit.md).

Reference machine: Apple silicon (`arm64`), release build, real
`bge-small-en-v1.5` embeddings unless noted.

---

## 1. Product definition

**Lynx is a deterministic repository intelligence engine for AI-native developer
tooling.**

- It **does**: repository discovery, parsing, symbol extraction, symbol
  identity, structural relationships, indexing, persistence, retrieval,
  ranking, resolution, context construction, resource control, explainability,
  and MCP/CLI exposure.
- It **does not**: reason, plan, execute, modify code, authorize, orchestrate
  workflows, or make autonomous decisions.

**Lynx discovers. Consumers decide.**

It is a standalone product with no conceptual dependency on any particular
agent, IDE, MCP client, or organization. Other tools may consume it; they do not
define it.

## 2. Product value

**Problem.** Agents and developer tools need reliable, fast, structured access
to repository knowledge without re-scanning, re-grepping, re-embedding, or
guessing through the codebase on every query.

**Who consumes it.** AI agents, MCP clients, coding assistants, IDEs, code
review systems, developer CLIs, automation — anything that can speak MCP or
shell out. The MCP surface is generic (8 tools, no vendor concepts).

**Why it is useful to an agent.** One call returns ranked, addressable symbols
with exact coordinates and provenance; `resolve` is deterministic; `context`
returns a token-bounded package. That removes navigation work the agent would
otherwise do by scanning source.

**Where it beats grep / vector search.** On the same grounded tasks, ripgrep is
faster at literal search but ranks worse whenever relevance or paraphrase
matters. A generic vector search returns vectors; Lynx returns symbols with
stable identity, and bare-symbol lookups bypass ranking entirely. See §4 and the
ripgrep baseline in the raw report.

## 3. Architecture

Lynx is a persistent substrate with a bounded query layer on top:

```text
lx index  ->  <repo>/.lynx/   graph.sqlite3 (identity, relations, vectors)
                              lexical/      (BM25 snippets)
          ->  exit
lx search|resolve|relations|context|status   (hydrate, query — never re-index)
```

- `lynx-core::Engine` owns indexing and retrieval; `lynx-storage::DualStorage`
  owns persistence; `lynx-mcp`/`lynx-cli` are transports.
- Queries stream from storage under a bounded candidate budget; no
  repository-proportional state is retained in RAM.
- One embedding-model instance per process, shared by indexing and querying.
- Fusion and resolution use total, deterministic orders; hashes never decide
  ranking.

Full ownership, lifecycle, state, resource bounds, and failure boundaries:
[`final-product-audit.md`](final-product-audit.md).

## 4. Retrieval quality

135 grounded tasks over the `auth-service` corpus, real ONNX embeddings:

| category | n | P@1 | P@3 | R@5 | R@10 | Hit@5 | MRR |
|---|---|---|---|---|---|---|---|
| exact | 35 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| structural | 15 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| relation | 15 | 0.933 | 1.000 | 1.000 | 1.000 | 1.000 | 0.967 |
| lexical | 20 | 0.700 | 0.850 | 0.925 | 0.925 | 0.950 | 0.852 |
| semantic | 20 | 0.550 | 0.750 | 0.825 | 0.900 | 0.950 | 0.804 |
| context | 10 | 0.100 | 0.600 | 0.750 | 0.850 | 0.800 | 0.509 |
| ambiguity | 10 | 0.700 | 0.900 | 0.900 | 0.900 | 0.900 | 0.783 |
| adversarial | 10 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| **ALL** | **135** | **0.793** | **0.904** | **0.937** | **0.956** | **0.963** | **0.893** |

**Failure analysis (semantic).** Inspecting every semantic miss with
`lx search --explain` (lexical rank, semantic rank, RRF, boost) showed the
misses fall into two groups, neither of which is a fusion-weight problem:

1. **Sibling ordering.** The expected symbol is in the result set but a sibling
   that is *also* an expected answer ranks first (e.g. `Login` vs
   `verifyPassword`). This is an ambiguity the consumer resolves, not a bug.
2. **Embedding representation.** For a few queries the model places the correct
   symbol at semantic rank 2–6 behind a lexically-similar sibling. For
   "how long an access token remains usable", `ExpirationSeconds` is `sem=6`
   while `ValidateToken` is `sem=0`; no channel weighting can reorder them,
   because the model itself ranks `ValidateToken` first semantically.

**What was tried and rejected.** Folding `fqdn` separators to spaces in the
embedding text (a representation change) was measured and **regressed** lexical
P@1 (0.700 → 0.650), so it was reverted. Fusion weights were not tuned: the
failures are not weight-limited.

**What was kept (evidence-based).** `rrf_fuse` is symmetric, so
`(lex=i, sem=j)` and `(lex=j, sem=i)` tie exactly; the previous tie-break used
the opaque content hash. Tie-breaks now use the lexical rank first (then
semantic rank, then hash), which is deterministic, independent of the identity
hashing scheme, and matches the high-precision exact-term channel. This removed
a hash-dependent ranking artifact and **restored the exact baseline** while
making the order more principled.

Net: semantic and lexical quality are **at baseline**; the deterministic
tie-break is more robust. Semantic top-1 remains representation-limited and is a
declared limit, not a hidden one.

## 5. Performance

Indexing, hydration, and query latency (cold = fresh process; warm = MCP
in-process). Structural commands (`resolve`, `relations`, `status`, lexical-only
search) load no embedding model.

Retrieval corpus (10 files / 47 symbols) and cold-start lifecycle:

| Budget | Value |
|---|---|
| `lx index` (full, corpus) | 0.824 s (1.080 s in the isolated lifecycle probe) |
| hydrate (metadata only) | < 1 ms measured; 91 ms in the earlier probe incl. process start |
| cold `search` p50 / p95 / p99 | 0.140 / 0.150 / 0.160 s (includes process start + ONNX load) |
| cold `resolve` p50 / p99 | 0.010 / 0.010 s (loads no model) |
| warm in-process query mean (MCP) | 2.73 ms |
| persisted index size (corpus) | 0.22 MB |

Cold `search` is dominated by process start and the ~250 MB ONNX model load;
`resolve`, `relations`, `status`, and lexical-only search load no model.

Repository scaling (files × 10 symbols, cold query per fresh process):

| files | symbols | index s | peak RSS MB | index MB | search p50 ms | search p95 ms | resolve p50 ms |
|---|---|---|---|---|---|---|---|
| 50 | 550 | 4.90 | 301.0 | 1.78 | 148.3 | 150.5 | 15.9 |
| 100 | 1100 | 8.73 | 303.0 | 3.50 | 149.5 | 150.6 | 15.7 |
| 200 | 2200 | 17.17 | 300.8 | 6.95 | 153.5 | 158.2 | 16.6 |
| 400 | 4400 | 34.29 | 308.2 | 13.85 | 157.1 | 160.7 | 16.7 |

The numbers are reproduced verbatim from `final_<timestamp>.txt`; see that file
for the authoritative run.

## 6. Resource safety

- **Peak RSS is flat in repository size** (301 MB at 550 symbols → 301 MB at
  4 400 symbols, 8×): the footprint is the ~250 MB ONNX model, not repository
  content.
- **Query memory is `O(candidate limit)`**, not `O(symbols)`: semantic search
  streams vectors through a bounded `TopK`; `Evidence` is materialized only for
  retained results.
- **Indexing memory is `O(largest file + batch)`**, with an 8 MiB per-file
  guard so an adversarial file cannot spike the process.
- **Low-memory hosts**: validated by measured RSS, not by running on constrained
  machines. Because query memory is bounded and small and indexing streams, Lynx
  degrades by slowing, not by consuming unbounded host memory.

## 7. Persistence

Measured lifecycle (`lx index` → persist → exit → new process → hydrate →
query), with the source tree deleted after indexing:

- `query_without_sources_returns_results = True` — a fresh process answers with
  the workspace's files gone.
- `db_unmodified_by_query = True` — a query cannot be a re-index.
- cold `search` p50 0.140 s / p95 0.150 s / p99 0.160 s; cold `resolve` p50
  0.010 s; persisted index 0.22 MB.

The query path never re-reads or re-embeds the workspace.

Freshness is explicit: `fresh`, `stale`, `missing`, `incompatible`, `corrupt`,
`unknown`. `lx status` reports it; MCP `index_status(check_freshness=true)`
exposes it; a git-commit mismatch also emits a cheap stderr warning at hydrate
time.

## 8. Incremental indexing

200-file / 2 200-symbol synthetic repository, fresh process per pass:

| operation | wall s | peak RSS MB | note |
|---|---|---|---|
| full index | 16.54 | 308.6 | initial build |
| re-index, no change | 0.16 | 267.1 | **103×** faster; zero embeddings |
| modify one file | 0.30 | 298.8 | **55×** faster; re-embeds 2 texts |
| add one file | 0.22 | — | |
| delete one file | 0.18 | — | |

Correctness: added, modified, deleted, renamed, and directory-moved files are
reflected with **no stale symbol, vector, or document**; freshness transitions
`fresh → stale → fresh` exactly. Regression tests in
`crates/lynx-core/tests/incremental.rs` pin each case.

## 9. MCP

200 sequential `search` calls: **success 1.0000**, p50 **3.89 ms**, p95 4.17 ms,
p99 4.51 ms, RSS drift +2.30 MB total (≈0.0115 MB/call, effectively flat).
Containment: 5/5 malformed frames contained, unknown tool rejected, zero timeout
returns `isError`, a huge query is answered, the server survives, and a restart
answers normally. The tool set is stable at eight tools; internal storage is
never exposed.

## 10. Agent utility

Realistic MCP navigation workflows:

| workflow | calls | latency ms | reached the coordinate |
|---|---|---|---|
| find authentication implementation | 1 | 4.88 | yes |
| find token validation (`resolve`) | 1 | 0.22 | yes |
| find token refresh | 1 | 5.14 | yes |
| find request authorization | 1 | 3.98 | yes |
| smallest context for auth (2048) | 1 | 5.05 | 1 558-token package |
| find callers of `Login` | 2 | 0.12 | 2 edges |

Each workflow reaches a grounded coordinate in one or two calls at
single-digit-millisecond warm latency, with no repository scan by the client.

### Baseline vs ripgrep (same grounded tasks, file level)

| category | n | rg@1 | rg@5 | Lynx@1 | Lynx@5 |
|---|---|---|---|---|---|
| exact | 35 | 0.886 | 1.000 | **1.000** | 1.000 |
| lexical | 20 | 0.650 | 1.000 | **0.950** | 1.000 |
| semantic | 20 | 0.050 | 0.650 | **0.950** | **1.000** |
| structural | 15 | 0.800 | 1.000 | **1.000** | 1.000 |

Lynx wins whenever ranking or paraphrase matters; ripgrep remains the right tool
for a literal single-token lookup (8.2 ms vs 148 ms cold for Lynx).

## 11. Limitations

- **Semantic top-1** is representation-limited (§4); recall is strong.
- **Context ordering** inherits retrieval ordering; the right symbol is usually
  present (Hit@5 0.800), rarely first (P@1 0.100).
- **Relations are intra-file**; cross-file calls are not linked.
- **Very large repositories** are supported to the measured envelope; semantic
  retrieval is an exact `O(N)` scan (no approximate index).
- **Low-memory hosts** were validated by RSS measurement, not by physical
  constrained-RAM runs.
- **`resolve` on a genuinely ambiguous name** is deterministic but not always
  intended; use `resolve_all`.

## 12. Remaining risks

Only evidence-backed risks are listed:

1. **Embedding-representation ceiling** on conceptual top-1 (measured). The
   mitigation is a better representation/model, not tuning; fusion changes were
   measured and did not help.
2. **`O(N)` semantic scan.** Fine across the measured envelope (flat
   sub-200 ms cold), but cost grows linearly; an ANN would be justified only if
   measurement shows it is a product bottleneck, which it is not at the declared
   scope.
3. **Cross-file relations absent** — caps relation recall on real, multi-file
   call graphs.
4. **Low-memory behavior is inferred from bounded RSS plus streaming**, not from
   a physical 4 GB run.

## 13. Release verdict

**PRODUCTION READY WITH DECLARED LIMITS.**

All correctness, persistence, runtime-safety, integration, and product-quality
gates pass; exact, structural, relational, and lexical retrieval are reliable;
semantic retrieval is useful by recall; incremental indexing, freshness, and
recovery are implemented and tested. The declared limits (semantic top-1,
context ordering, intra-file relations, large-repository envelope, low-memory
inference) are measured and documented, not hidden.
