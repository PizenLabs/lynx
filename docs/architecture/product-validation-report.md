# Lynx Product Validation Report

Result of the validation pass described in
[`product-validation-audit.md`](product-validation-audit.md). Every number below
is reproducible with `benchmarks/scripts/run_validation.sh`; raw runs are under
`benchmarks/reports/validation_<timestamp>.txt`.

"Before" = commit `0064c37`; "after" = this pass. Machine: Apple silicon,
release build.

---

## 1. Executive summary

**What Lynx does well.** Exact symbol resolution, structural discovery, and
relation queries are deterministic and effectively perfect on the grounded
suite (Precision@1 1.000, 1.000, 0.933). Lexical and semantic retrieval are
genuinely useful at symbol granularity (real-embedding Precision@1 0.700 and
0.550; Hit@5 0.950 for both). Every one of the 135 tasks completes without error,
including empty, hostile, oversized, and Unicode query inputs.

**What it did poorly.** Before this pass, Lynx had **no hydration**: every
`lx search` / `resolve` / `relations` / `context` re-indexed the entire
repository in-process, and every snippet and vector was retained in RAM for the
process lifetime. `lx search` on a 1 100-symbol repo cost **8.63 s and 315 MB**;
`lx resolve` cost the same because it too re-indexed. The persistent `.lynx`
artifact was write-only.

**What changed.** The engine is now a thin query layer over the persisted
substrate. `Engine::new` hydrates; queries stream from SQLite/Tantivy and
materialize only the retained results. Two correctness bugs were fixed as a
result: identical source bodies colliding under the `content_hash` primary key
(silent symbol loss) and malformed lexical queries failing the whole query.

**Is it production-ready?** For small and medium repositories (≤ ~50 MB /
~10 k symbols) with a stable workspace: **yes, a production candidate**. For
continuously-mutating or very large repositories: **not yet** — there is no
incremental indexing and no demonstrated 500 MB+ behaviour. Details in
[`production-readiness.md`](production-readiness.md).

---

## 2. Product value

- **Who can use it.** Any consumer that needs repository coordinates without
  owning a codebase-shaped index: AI agents, IDEs, MCP clients (Claude Desktop et
  al.), code-review systems, developer CLIs, automation. The MCP surface is
  generic (8 tools, no PizenLabs concepts) and the CLI is self-contained.
- **What problem it solves.** It converts intent ("where is token refresh
  handled?") into deterministic coordinates (fqdn, file, line range, snippet)
  that survive across runs and processes, without the consumer scanning source.
- **When it beats ripgrep.** Measured on the same 90 grounded tasks at the file
  level:

  | category | n | rg@1 | Lynx@1 | rg@5 | Lynx@5 |
  |---|---|---|---|---|---|
  | exact | 35 | 0.886 | **1.000** | 1.000 | 1.000 |
  | lexical | 20 | 0.650 | **0.950** | 1.000 | 1.000 |
  | semantic | 20 | 0.050 | **0.950** | 0.650 | **1.000** |
  | structural | 15 | 0.800 | **1.000** | 1.000 | 1.000 |

  Lynx wins decisively whenever ranking or paraphrase matters; ripgrep is faster
  (9.6 ms vs 152 ms cold) and remains the right tool for a literal single-token
  lookup. Lynx is not a slower ripgrep — it answers a different question at a
  different granularity.
- **When it beats generic vector search.** Lynx never puts a vector in front of
  the consumer: results are symbols with exact coordinates, and bare-symbol
  queries bypass ranking entirely (deterministic, P@1 1.000).
- **Why an agent benefits.** One call returns ranked, addressable symbols with
  provenance and (for `context`) a token-bounded package, so an agent needs fewer
  iterations and inspects fewer irrelevant files.
- **Why MCP matters.** The same eight primitives are exposed to any MCP client
  over stdio, with per-call timeouts and contained failures — no SDK, no
  PizenLabs coupling.

---

## 3. Retrieval results

Real `bge-small-en-v1.5` embeddings, 135 grounded tasks. (Hermetic fake-provider
run, for regression only, is in the raw report.)

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

**Failure categories.**
- *Context ordering* (P@1 0.100 but Hit@5 0.800): the correct symbol is usually
  in the 16-item package but rarely first. This is a ranking problem inside the
  context compiler, not an indexing problem.
- *Ambiguity* (P@1 0.700): `resolve_all("Login")` exposes every candidate, but
  the deterministic primary is the lexicographically smallest production symbol
  (`auth::AdminService::Login`), which is not always the intended one. Lynx no
  longer *hides* the ambiguity; it cannot *resolve* genuinely ambiguous intent.
- *Semantic paraphrase* (P@1 0.550): short paraphrases with little lexical
  overlap sometimes rank a sibling ahead of the target; recall is strong.
- *Lexical* (P@1 0.700): multi-word queries whose best answer shares no literal
  token (e.g. "bearer header" → `bearerToken`) rely on the semantic channel.

Behavioural/flow queries ("how does authentication work", "where does a request
become authenticated") were tested as the `semantic` and `context` categories;
they succeed by recall, not by top-1. This is the weakest area of the product.

---

## 4. Performance

| Budget | Before | After |
|---|---|---|
| `lx index` (10 files / 47 symbols, real) | — | 1.14 s |
| `hydrate` (real, includes ONNX load) | N/A (no hydrate) | 0.094 s |
| cold `lx search` p50 / p95 / p99 | 8.63 s (re-index) | **0.140 s** / 0.150 / 0.150 |
| cold `lx resolve` p50 / p99 | ~8.6 s (re-index) | **0.010 s** / 0.010 |
| warm in-process query p50 / p95 / p99 (MCP) | — | **3.89 ms** / 4.30 / 4.64 |
| in-process mean query (135 tasks, real) | — | 2.87 ms |
| persisted index size (auth-service) | 0.22 MB | 0.22 MB |

Cold CLI latency includes process start and ONNX model load; `resolve`,
`relations`, and `status` use a provider-free engine and load no model.

Indexing scaling (files × 10 symbols, synthetic Rust):

| files | before wall / peak RSS | after wall / peak RSS |
|---|---|---|
| 50 | 5.01 s / 295 MB | 5.56 s / 295 MB |
| 100 | 9.24 s / 307 MB | 9.70 s / 305 MB |
| 200 | 17.07 s / 313 MB | 17.58 s / 317 MB |
| 400 | 33.70 s / 311 MB | 35.38 s / 307 MB |

**Regression, reported honestly:** indexing wall time increased ~5 % because
embedding vectors are now written per file (an extra SQLite transaction per file)
instead of being accumulated and written once at the end. This is the deliberate
trade that makes indexing memory flat and enables persistence. Peak RSS is
unchanged or slightly better and no longer grows with symbol count (see §5).

---

## 5. Resource safety

- **Indexing memory is flat with repository size.** Auth-service: 320 MB at
  1 100 symbols → 330 MB at 8 400 symbols (8× the symbols, +10 MB). Before, the
  same range grew 313 MB → 358 MB. The remaining footprint is dominated by the
  ~250 MB ONNX model, not by repository content.
- **Query memory is `O(candidate limit)`.** Semantic search streams vectors from
  SQLite one at a time through a bounded top-k heap; `Evidence` is materialized
  only for retained results, and snippets are fetched from Tantivy for exactly
  those results.
- **Persistence proof.** With the source tree deleted after indexing, a fresh
  process still returns results
  (`query_without_sources_returns_results=True`), and the database file is not
  modified by a query (`db_unmodified_by_query=True`). A query cannot be a
  re-index.
- **No uncontrolled host-memory growth** on any tested input; candidate counts
  are hard-capped (`clamp(limit·20, 100, 2000)`, caller override max 100 000).
- **Open risk:** indexing has no hard ceiling; an adversarial single huge file can
  still spike. Subprocess isolation with an OS limit remains the recommendation
  for untrusted input. Very large repositories (500 MB+) are unmeasured.

---

## 6. Lifecycle

| Step | Behaviour |
|---|---|
| `lx index <repo>` | full reset + rebuild into `<repo>/.lynx`; streams per file |
| persist | SQLite (WAL) + Tantivy + vectors on disk |
| process exit | — |
| restart | `Engine::new` hydrates provenance + counts; no file read, no re-embed |
| query | served from the persisted index; never re-indexes |
| update / re-index | `lx index --force` resets stale rows/vectors/documents and rebuilds |
| `lx mcp` | hydrates at startup; builds once only if absent |

Regression coverage: `crates/lynx-core/tests/persistence.rs` proves
(a) reopening answers without the workspace, (b) hydration is deterministic
across reopens and performs zero embedding work, (c) add/delete/rename are
reflected after re-index with no stale symbols, (d) ambiguity is surfaced and
noise demoted.

---

## 7. MCP

200 sequential `search` calls: **success 1.0000**, p50 **3.89 ms**, p95 4.30 ms,
p99 4.64 ms, RSS +2.05 MB total (≈0.010 MB/call, i.e. flat). Containment: 5/5
malformed frames contained, unknown tool rejected, zero timeout returns
`isError`, a 10 000-token query is answered, and the server survives a restart.
All diagnostics go to stderr; stdout carries only protocol frames.

---

## 8. Agent utility

The design goal is that a consumer needs the fewest possible calls and the least
context. On the auth-service flow, a "find where JWT authentication is
implemented" workflow is:

1. `search "jwt authentication"` → `auth::ParseJWT`, `auth::ValidateToken`,
   `auth::AuthService::Login` (ranked, with coordinates) — 1 call, ~3.9 ms warm;
2. `context "jwt authentication"` → a ≤2048-token package whose Hit@5 is 0.800,
   i.e. the relevant symbol is present at a bounded token cost;
3. `relations <hash>` → the call graph around the chosen symbol.

Versus ripgrep, this replaces keyword scanning + manual symbol localization with
one ranked, provenance-pinned result set. The measured limit is context
*ordering* (P@1 0.100), which is the main remaining lever for reducing the
number of results an agent inspects.

---

## 9. Remaining risks

1. **No incremental indexing** — full rebuild per pass; no staleness detection.
2. **Semantic scan is `O(N·D)` per query** (memory `O(k)`); fine at ~10 k
   symbols, unproven at 500 MB.
3. **Context ordering** — right symbol present (Hit@5 0.800) but rarely first.
4. **No hard indexing memory ceiling** for adversarial inputs.
5. **Relations are intra-file** — cross-file calls are not linked.
6. **Ambiguous `resolve` is deterministic but not always intended**;
   `resolve_all` is the correct surface.
7. **Very large / cross-language monorepos unmeasured.**

---

## 10. Verification

```bash
cargo fmt --all -- --check          # pass
cargo check --workspace             # pass
cargo test --workspace              # pass (81 tests)
cargo clippy --workspace --all-targets --all-features -- -D warnings   # pass
benchmarks/scripts/run_validation.sh
```

Raw reports: `benchmarks/reports/validation_<timestamp>.txt` (this pass:
`validation_20261016_062500.txt` and the post-fix rerun
`validation_20261016_062841.txt`).
