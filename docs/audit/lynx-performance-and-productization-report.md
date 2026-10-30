# Lynx performance and productization report — 2026-10-30

Scope: a measurement-driven optimization pass over the existing Lynx codebase
(`b87588b`, branch `perf/optimize-logic`). The goal was to make repository
discovery faster and lighter **without** expanding Lynx into GRY, changing the
protocol, or trading retrieval quality for microbenchmarks.

Raw evidence (generated, not committed):

- Baseline: `benchmarks/reports/baseline_20261030_154835.md`
- Final: `benchmarks/reports/final_20261030_161950.md`
- Baseline method detail: [`docs/audit/baseline.md`](baseline.md)

---

## A. Executive summary

### Initial state

- Every `lx search`, **including `--mode lexical`**, constructed the ONNX
  embedding model: **766 ms** and about **600 MB** before the first lexical
  result.
- Every `lx mcp` process loaded the model at startup, even for clients that only
  used `resolve`, `relations`, or `index_status`: **591 MB** idle RSS.
- A no-change `lx index` loaded the model and re-read every file twice:
  **0.77 s / 594 MB**. A deletion-only pass cost **0.81 s** with the model
  loaded, despite computing no embeddings.
- Each grammar-backed file was parsed and analyzed twice (once for symbols,
  once for relations).
- Every command spawned `git` up to twice to discover the workspace root and
  `HEAD`, dominating the cold latency of otherwise millisecond operations.
- Candidate selection in indexing used repeated `Vec::contains`, an `O(files²)`
  scan on a full build.
- CamelCase identifiers were opaque to BM25 (`verifyPassword` was one token), so
  natural lexical queries such as `password check` could not match them.

### Final state

| Dimension | Baseline | Final | Change |
|---|---:|---:|---:|
| Cold lexical search (`lx search --mode lexical`) | 766 ms | **8.8 ms** | **−98.9 %** |
| Cold exact resolve (`lx resolve`) | 13.5 ms | **8.5 ms** | **−37 %** |
| Cold status (`lx status --no-check`) | 13.7 ms | **8.3 ms** | **−39 %** |
| Cold hybrid search (model load included) | 823 ms | 801 ms | −2.7 % |
| No-change index, 200 files | 0.77 s | **0.02 s** | **−97 %** |
| No-change index peak RSS | 594 MB | **12.0 MB** | **−98 %** |
| Deletion-only index, 200 files | 0.81 s | **0.05 s** | **−94 %** |
| MCP RSS immediately after `initialize` (existing index) | 591 MB | **11.1 MB** | **−98 %** |
| Warm lexical query (in-process MCP) | 0.13 ms | 0.14 ms | +0.01 ms |
| Warm exact resolve (in-process MCP) | 0.04 ms | 0.04 ms | no change |
| Retrieval quality, all 135 tasks, P@1 | 0.793 | **0.815** | **+2.2 pts** |
| Retrieval quality, all 135 tasks, R@10 | 0.956 | **0.981** | **+2.5 pts** |
| Retrieval quality, all 135 tasks, MRR | 0.893 | **0.911** | **+1.8 pts** |

A no-change index is now **≈2 400×** faster than a full build on the 200-file
synthetic workspace. On the same machine and build profile, the model-free path
is the product's default for structural, lexical, status, and idle MCP use; the
model is paid only when a semantic vector is actually required.

### Main improvements

1. Lazy embedding-model construction, shared by CLI, MCP, and indexing.
2. Model-free MCP startup and true single-open session bootstrap.
3. One workspace read per index pass instead of two.
4. Single-pass parser: one tree + one analysis per file.
5. Filesystem-based git discovery (no subprocess on the startup path).
6. Prepared SQLite statements for per-row inserts and scans.
7. Linear-time changed-file selection.
8. Identifier word expansion in the lexical index, improving real retrieval
   quality while keeping exact matching intact.
9. Bounded embedding batches, fixing a dense-file memory blow-up (31 GB → 2.3 GB
   on this repository) while keeping throughput within ≈11 %.
10. A reproducible warm-query probe and parser microbenchmark added to the
    benchmark suite.

### Remaining limitations

- The first semantic operation in a fresh process still pays the model load
  (≈800 ms and ≈600 MB on the reference machine). That is the deliberate price
  of not making model-free commands pay it.
- Semantic retrieval is still an exact `O(symbols × dimensions)` scan. At 4 400
  symbols the scan adds ≈8 ms; at 100 000 it would add hundreds of milliseconds.
  No ANN index was added because the measured need did not justify the
  complexity and quality risk.
- Full indexing is embedding-bound: 200 files took 47.2 s and 1.09 GB peak RSS
  on this environment. The bounded embedding batch trades ≈11 % throughput for a
  much lower memory ceiling (and fixes a 31 GB dense-file blow-up). Parser/SQL
  improvements are real but small relative to ONNX inference.
- `compute_file_snippets` remains `O(symbols × file bytes)` for pathological
  dense files, bounded by the 8 MiB per-file guard.
- No direct energy measurement was performed.

---

## B. Architectural audit

The engine already had strong invariants (bounded candidates, streaming semantic
scan, deterministic resolution, `O(k)` top-k). The audit therefore targeted
work that was either unconditional, duplicated, or accidentally expensive.

| # | Finding | Location | Root cause | Frequency | Fix |
|---|---|---|---|---|---|
| 1 | Model constructed for lexical-only search | `lynx-core`/`lynx-mcp::Session::hydrate` | eager `FastEmbedProvider` in every session | every `search` | `LazyFastEmbedProvider`; model only on `embed_*` |
| 2 | Model constructed for every MCP process | `lynx-mcp::run` | eager provider at startup | every MCP session | lazy provider |
| 3 | Model constructed for no-change/deletion indexing | `Session::update` | eager provider | every `lx index` | lazy provider; zero embeddings → zero model |
| 4 | Workspace read twice per index | `engine::resolve_snapshot` + `scan_workspace` | separate `tree_hash` walked and read all files | every index pass | derive snapshot hash from the scan |
| 5 | Two parses + two analyses per file | `Parser::parse`, each adapter | `parse_symbols` built a tree; facade built another | every indexed file | `LanguageAdapter::parse_all` override per grammar adapter |
| 6 | Two `git` processes per CLI command | `session::git_toplevel`, `git_head` | shelling out for filesystem facts | every CLI/MCP hydrate | ancestor walk for `.git`; direct `HEAD`/loose-ref/`packed-refs` read; subprocess only as rare fallback |
| 7 | Substrate opened twice at MCP startup | `Session::open_with` + `storage_is_populated` | probe engine then real engine | every MCP start | one engine; build only when `symbol_count == 0` |
| 8 | `O(files²)` changed-file filter | `Engine::update_repository` | `plan.added.contains` / `plan.modified.contains` per file | full build | `HashSet<&str>` membership |
| 9 | Per-row statement preparation | `GraphStore` insert methods | `conn.execute` per row | every symbol/relation/embedding | `prepare_cached` inside the transaction |
| 10 | Per-comparison clones in resolution sort | `Engine::ranked_matches` | comparator cloned `fqdn` and hash | ambiguous names | borrow `&str` keys |
| 11 | CamelCase opaque to BM25 | `TantivyStore::add_documents` | default tokenizer does not split case | every lexical query | non-stored `code_tokens` field with camel/snake expansion |
| 12 | Model-free `similar` still scans all vectors | `Engine::similar_with` | exact scan by design | every `similar` | not changed; documented exact scan |
| 13 | Dense file could exhaust memory during embedding | `Engine::index_file` | all symbols of a file sent as one batch; runtime allocates `batch × sequence²` | files with many symbols | bounded batches of 4 texts; dense-repo peak fell from 31 GB (killed) to 2.3 GB |

### Findings evaluated and rejected

- **Prefetched query norm for cosine**: implemented, measured at 4 400 symbols
  (18.2 ms → 18.4 ms, i.e. no change), reverted. SQLite row/decode cost dominates
  the scan, not the norm arithmetic.
- **Tantivy field boosts** (`code_snippet ×2`, `file_path ×0.5`): produced
  byte-identical quality metrics on the 135-task suite; reverted.
- **Higher candidate caps or extra reranking**: no measured recall bottleneck at
  the current caps; would add cost and nondeterminism risk.
- **ANN/vector database**: 4 400-symbol semantic scan is 18 ms including query
  embedding; adding an index would add build cost, memory, and quality variance
  not justified by the current evidence.

### Architecture retained deliberately

- `lynx-core` still owns product logic; storage, parser, embed, protocol, and
  transports keep their boundaries.
- Relations remain intra-file. No cross-file graph was added.
- The serialized protocol was not changed. Index schema did change (see below),
  which is an index-format change, not a wire-contract change.
- `Engine` remains a thin query layer over persisted storage; no in-memory
  repository state was introduced.

---

## C. Benchmark results

### Environment

| Item | Value |
|---|---|
| OS | macOS Darwin 27.0.0, arm64 |
| Rust | rustc 1.99.1 (Homebrew), edition 2021 |
| Build | release, `opt-level=3`, `lto=true`, `codegen-units=1`, `strip=true` |
| Embedder | FastEmbed `bge-small-en-v1.5`, 384 dimensions |
| Quality corpus | `benchmarks/corpus/auth-service`, 10 files / 47 symbols / 44 relations |
| Synthetic scaling | `gen_repo.py`, Rust, 10 symbols/file |
| Samples | 20–50 cold CLI runs; 50–200 MCP calls; 3 hybrid cold runs |

> **Comparability warning.** The archived October 16 report on the same machine
> shows markedly lower absolute latency and model RSS. The OS, SDK, and ONNX
> Runtime changed between then and now. Every baseline/final pair below was
> produced on 2026-10-30 with the same harness, corpus, build profile, and
> machine. Absolute numbers should not be compared across dates.

### Cold CLI latency

| Operation | Baseline p50 | Final p50 | Change |
|---|---:|---:|---:|
| `lx --version` | 6.7 ms | 6.8 ms | noise |
| `lx resolve ValidateToken` | 13.5 ms | **8.5 ms** | **−37 %** |
| `lx search token --mode lexical` | 766 ms | **8.8 ms** | **−98.9 %** |
| `lx search "token validation"` (hybrid) | 823 ms | 801 ms | −2.7 % |
| `lx status --no-check` | 13.7 ms | **8.3 ms** | **−39 %** |

The process-spawn floor is ≈6.7 ms on this machine. After the change, resolve
and status spend ≈1.6 ms above that floor (storage open + counts + lexical
open), down from ≈6.8 ms.

### Warm in-process MCP latency (corpus, 50 samples)

| Operation | Baseline p50 / p95 | Final p50 / p95 |
|---|---|---|
| `resolve` | 0.04 / 0.06 ms | 0.04 / 0.07 ms |
| lexical search | 0.13 / 0.16 ms | 0.14 / 0.18 ms |
| semantic search | 9.83 / 10.30 ms | 9.81 / 10.37 ms |
| hybrid search | 9.88 / 10.09 ms | 9.99 / 10.36 ms |
| context | 8.62 / 8.92 ms | 8.07 / 8.56 ms |
| similar | 0.14 / 0.16 ms | 0.14 / 0.22 ms |

Semantic/hybrid latency is dominated by one query embedding (~10 ms on this
environment); the exact vector scan is negligible at 47 symbols. At 4 400
symbols, the scan adds ≈8 ms (18.4 ms total).

### Model-load boundary (the deliberate trade-off)

| Measurement | Baseline | Final |
|---|---:|---:|
| MCP RSS after `initialize` (indexed corpus) | 591 MB | **11.1 MB** |
| First semantic query in a fresh MCP process | 55 ms (model already loaded at startup) | 686 ms (model loads here) |
| Second and later semantic queries | 9.8 ms | 9.8–10.1 ms |
| Time to first result, semantic workflow | startup (~820 ms) + 55 ms | startup (~11 ms) + 686 ms |

Total time-to-first-semantic-result is the same order of magnitude and was
slightly lower in this run (≈875 ms baseline versus ≈697 ms final), but the
model cost moved from every startup to the first semantic request. Idle memory
and every model-free operation improve by one to two orders of magnitude. This
is the intended trade-off and is documented in the README, Concepts, CLI, MCP,
and Troubleshooting guides.

### Indexing

| Case | Baseline | Final | Change |
|---|---:|---:|---:|
| 50-file full index | 11.21 s | 12.62 s | +12.6 % |
| 200-file full index | 42.45 s | 47.24 s | +11.3 % |
| 400-file full index | 81.76 s | 93.43 s | +14.3 % |
| 200-file no-change index | 0.77 s | **0.02 s** | **−97 %** |
| 200-file one-file modified | 1.19 s | 1.10 s | −7.6 % |
| 200-file one-file added | 0.88 s | 0.86 s | −2.3 % |
| 200-file one-file deleted | 0.81 s | **0.05 s** | **−94 %** |
| Peak RSS, 200-file full index | 1.54 GB | **1.09 GB** | **−29 %** |
| Index size, 400 files / 4 400 symbols | 13.85 MB | 13.89 MB | +0.3 % |

Full indexing is inference-bound, and the batching change that fixed the dense-
file memory blow-up trades some throughput for a much lower ceiling: 200 files
run 11 % slower but peak at 1.09 GB instead of 1.54 GB, and a dense real
repository that previously peaked at 31 GB and was killed now peaks at 2.3 GB.

The incremental figures use a plain synthetic directory, not a git checkout.
On a real work tree, `resolve_snapshot` still runs `git rev-parse HEAD` and
`git status --porcelain` for provenance; that subprocess cost is separate from
the 0.02 s scan/diff. Query startup does not run git.

Parser single-pass measured **≈1.7×** faster in isolation across runs
(1.67–1.78×, `parser_bench`, 205 symbols / 406 relations):

| Parser path | Per file |
|---|---:|
| Production single pass | 3.3–3.5 ms |
| Historical two-pass composition | 5.7–5.8 ms |

### CPU time

Measured with `/usr/bin/time -l` on a 50-file synthetic workspace:

| Case | Baseline user+sys | Final user+sys | Change |
|---|---:|---:|---:|
| Full index | 28.78 s | 28.87 s | noise (ONNX-bound) |
| No-change index | 0.72 s | **0.02 s** | **−97 %** |

No energy measurement was performed, so no energy claim is made.

### Retrieval quality (135 grounded tasks, real embeddings)

| Category | n | Baseline P@1 | Final P@1 | Baseline R@10 | Final R@10 | Baseline MRR | Final MRR |
|---|---:|---:|---:|---:|---:|---:|---:|
| exact | 35 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| lexical | 20 | 0.700 | **0.750** | 0.925 | **1.000** | 0.852 | **0.910** |
| semantic | 20 | 0.550 | 0.550 | 0.900 | **0.925** | 0.804 | 0.800 |
| context | 10 | 0.100 | **0.300** | 0.850 | **1.000** | 0.509 | **0.647** |
| ambiguity | 10 | 0.700 | 0.700 | 0.900 | 0.900 | 0.783 | 0.783 |
| relation | 15 | 0.933 | 0.933 | 1.000 | 1.000 | 0.967 | 0.967 |
| structural | 15 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| adversarial | 10 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| **all** | **135** | **0.793** | **0.815** | **0.956** | **0.981** | **0.893** | **0.911** |

Regressions and caveats:

- Semantic **R@5** moved 0.825 → 0.800 and Hit@5 0.950 → 0.900, while R@10
  improved 0.900 → 0.925 and P@1 was unchanged. The lexical channel now returns
  more candidates for the same query, which can displace one semantic candidate
  at rank 5. Overall quality improved.
- The hermetic fake-provider run (stability only, semantically meaningless) moved
  P@1 0.659 → 0.652, MRR 0.763 → 0.758, while R@10 improved 0.911 → 0.944.
  These variations are expected because the lexical channel changed; they are
  not used as quality evidence.
- Exact resolution, ambiguity ordering, relations, and structural coordinates
  are unchanged and deterministic.

### MCP reliability

| Measurement | Baseline | Final |
|---|---:|---:|
| Sequential calls | 200 | 200 |
| Success rate | 1.0000 | 1.0000 |
| p50 / p95 / p99 | 8.11 / 8.41 / 8.57 ms | 8.04 / 8.29 / 8.60 ms |
| RSS after `initialize` | 591 MB | **11.1 MB** |
| RSS after warm-up | — | 626 MB |
| RSS growth over 200 calls | +5.8 MB | +2.80 MB |
| Malformed frames contained | 5/5 | 5/5 |
| Unknown tool / zero timeout / huge query | contained | contained |
| Restart | OK | OK |

The six realistic agent workflows all reached a grounded result. The first
semantic workflow paid the one-time ≈690 ms model load; the following semantic
workflows took 15–17 ms, and resolve/relations calls stayed below 0.2 ms. The
context workflow returned a bounded 16-item package.

### ripgrep comparison (same grounded tasks, file level)

| Category | rg@1 | lynx@1 | rg@5 | lynx@5 |
|---|---:|---:|---:|---:|
| exact | 0.886 | 1.000 | 1.000 | 1.000 |
| lexical | 0.650 | 0.950 | 1.000 | 1.000 |
| semantic | 0.050 | 0.950 | 0.650 | 0.950 |
| structural | 0.800 | 1.000 | 1.000 | 1.000 |

Lynx cold latency is higher than ripgrep because it hydrates an index and, for
hybrid queries, loads the model. The comparison is about result quality and
stable symbol coordinates, not a claim that Lynx is a faster grep.

---

## D. Implemented changes

### Files changed

| File | Change |
|---|---|
| `crates/lynx-embed/src/lazy.rs` (new) | `LazyFastEmbedProvider`: infallible construction, model on first `embed_*`, `is_initialized` for tests/diagnostics. |
| `crates/lynx-embed/src/lib.rs`, `fastembed.rs` | Export lazy provider; share the dimension constant. |
| `crates/lynx-mcp/src/session.rs` | Lazy provider in every session; one-engine `open_with`; filesystem git root/HEAD discovery with subprocess fallback; tests for worktrees and refs. |
| `crates/lynx-mcp/src/lib.rs` | Regression test: structural/lexical tools work with `UnavailableProvider`; semantic fails cleanly. |
| `crates/lynx-core/src/engine.rs` | Scan workspace once; derive snapshot hash from file digests; `HashSet` changed-file selection; allocation-free resolution sort key; bounded embedding batches. |
| `crates/lynx-parser/src/adapter.rs` | `parse_all` implementation per grammar adapter; shared relation builders; borrow `container_requests`. |
| `crates/lynx-parser/src/lib.rs` | Facade uses `parse_all`; equivalence test versus the old composition. |
| `crates/lynx-parser/examples/parser_bench.rs` (new) | Reproducible single-pass vs two-pass benchmark. |
| `crates/lynx-storage/src/graph_store.rs` | Schema version 4; cached prepared statements for inserts, file states, and embedding scans. |
| `crates/lynx-storage/src/tantivy_store.rs` | Schema marker + derived-index migration; non-stored `code_tokens` field; camel/snake identifier expansion; tests. |
| `benchmarks/scripts/warm_query_probe.py` (new) | Per-primitive warm latency and lazy-model RSS boundary. |
| `benchmarks/scripts/mcp_stress.py` | Warm-up before steady-state RSS/slope measurement; reports startup RSS separately. |
| `benchmarks/scripts/run_final.sh` | Runs the warm-query probe. |
| `benchmarks/README.md` | Documents the new probes. |
| `README.md`, `docs/cli.md`, `docs/concepts.md`, `docs/mcp.md`, `docs/troubleshooting.md` | Correct model-initialization behavior. |
| `docs/architecture/production-contract.md`, `production-readiness.md` | Updated resource invariant and measured gate numbers; removed the stale “incremental indexing is not implemented” claim. |
| `docs/audit/baseline.md` (new) | Baseline method, raw numbers, and known limitations. |

### Index schema change

The lexical index gained a non-stored `code_tokens` field. A `schema_version`
marker in `.lynx/lexical` causes an old lexical directory to be discarded, and
the graph schema version was bumped from 3 to 4 in lockstep so an old index
reports `incompatible` and is reset instead of serving a populated graph beside
an empty lexical index. This is derived data; `lx index` (or `--force`) rebuilds
it. The serialized protocol is untouched.

### Tests added

- Lazy provider construction does not initialize the model.
- Structural and lexical MCP tools work with no embedding provider; semantic
  returns a tool error.
- `parse_all` produces exactly the symbols and relations of the old two-pass
  composition for Rust, Go, TypeScript, JavaScript, and Python.
- Workspace-root discovery finds an enclosing `.git` directory.
- `HEAD` is resolved from loose refs, `packed-refs`, detached HEAD, and a
  linked worktree's common refs without spawning git.
- CamelCase/snake_case expansion and lexical search by constituent words.
- Incompatible lexical schema is discarded.
- Dense file embeddings are split into bounded batches while every symbol is
  still embedded exactly once.
- Existing incremental, lifecycle, persistence, corruption, wire-contract, CLI,
  and MCP tests remain green.

---

## E. Resource optimization

### Model initialization

`LazyFastEmbedProvider` holds `Mutex<Option<FastEmbedProvider>>`. The first
`embed_query`/`embed_batch` constructs the model under the lock; concurrent
callers wait rather than constructing twice. Initialization failure is retried
on the next call instead of poisoning the provider.

Measured effects:

- `lx search --mode lexical`: model never constructed; 12.5 MB peak RSS.
- `lx resolve`, `relations`, `status`, `doctor`, `--version`: unchanged
  model-free behavior.
- `lx mcp` startup on an existing index: 11.1 MB until the first semantic
  request. A first-use build still embeds and loads the model.
- No-change `lx index`: no model, 12.0 MB peak RSS. A deletion-only pass also
  computes no embeddings, so it does not construct the provider.

### Memory

- No-change indexing peak RSS: 594 MB → 12.0 MB.
- MCP startup RSS: 591 MB → 11.1 MB.
- 200-file full index peak RSS: 1.54 GB → 1.09 GB.
- A dense real repository (162 files / ≈1 600 symbols in this project)
  previously peaked at 31 GB and was killed; with bounded batches it indexes in
  ≈62 s at ≈2.3 GB peak.
- Steady-state hybrid RSS is still model-dominated (~600 MB) and flat in
  repository size.
- Index size grew 0.3 % (13.85 → 13.89 MB at 4 400 symbols) for identifier
  expansion.
- No per-query repository-proportional state was introduced.

### CPU

- No-change indexing CPU: 0.72 s → 0.02 s.
- Parser: ≈1.7× faster in isolation (1.67–1.78× across runs).
- Query CPU is unchanged; warm latencies stayed within noise.
- No new threads, background tasks, or watchers.

### Database and I/O

- One workspace read per index pass instead of two.
- Prepared statement reuse for per-row inserts and repeated scans.
- Linear-time changed-file selection.
- Transactions and durability semantics unchanged (`WAL`, `synchronous=NORMAL`,
  foreign keys, per-file transactions).

### Incremental indexing

- Unchanged workspace: no parse, no embed, no lexical writer, no model.
- Single changed file: only that file is parsed and embedded (existing test
  asserts exactly two texts embedded for two symbols).
- Deletion-only: no embed, no model.
- Freshness lifecycle still `fresh → stale → fresh`.

### MCP

- Startup no longer loads the model.
- Request dispatch, tool registration, stdout/stderr separation, cancellation,
  timeouts, and malformed-frame containment are unchanged.
- Steady-state RSS slope over 200 calls: +0.014 MB/call (2.80 MB total), within
  allocator noise and lower than the baseline measurement.

---

## F. Quality verification

### Commands executed

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 scripts/docs/check_links.py
tests/distribution/release_test.sh
tests/distribution/install_test.sh
```

### Results

- `cargo fmt --all -- --check`: pass.
- `cargo check --workspace`: pass.
- `cargo test --workspace`: pass. 125 tests: 35 core unit + 23 core integration
  (incremental, lifecycle, persistence, corruption), 16 storage, 19 MCP, 8
  parser, 14 protocol + 3 wire-contract, 1 embed, and 6 CLI; no failures.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `python3 scripts/docs/check_links.py`: pass (48 Markdown files).
- `tests/distribution/release_test.sh`: pass (tag classification, version
  guards, release-workflow gating, unsupported targets).
- `tests/distribution/install_test.sh`: pass (fresh/repeat install, upgrade,
  downgrade refusal, checksum failure, missing artifact, atomic replacement,
  download-only).

### Build-environment note (not a repository change)

On this machine, the current macOS SDK no longer resolves an ONNX Runtime
Objective-C symbol (`___isPlatformVersionAtLeast`) through the default link
line. Release binaries were built by passing the Xcode `libclang_rt.osx.a`
archive explicitly to the final link (`-C link-arg=.../libclang_rt.osx.a`).
This is an environment/SDK issue, not a Lynx dependency change; no workaround
was committed to the repository.

### Unverified areas

- No direct energy measurement.
- No multi-machine or Windows/Linux measurement in this pass (macOS arm64 only).
- No 100 000-symbol semantic benchmark; the scan cost is extrapolated from
  4 400 symbols and is labelled as extrapolation.
- The `cargo install` and clean-machine distribution scripts were not run; the
  release and installer decision paths were.
- Retrieval quality is measured on the checked-in 135-task corpus, not on
  arbitrary repositories.

---

## G. Remaining work

1. **Semantic scan at very large symbol counts.** Exact scan is 18.4 ms at 4 400
   symbols (including query embedding), of which ≈8 ms is scan. At 100 000
   symbols this becomes hundreds of milliseconds. An ANN index (or a compact
   candidate filter) should be considered only with a measured target and a
   quality evaluation; it is not required for the current envelope.
2. **First semantic call in a fresh process.** Model load is ≈800 ms / ≈600 MB on
   this environment. Options: keep the lazy boundary (current), allow an explicit
   `--warm`/preload flag, or share a long-lived process. A daemon is out of scope
   for Lynx's product boundary.
3. **Snippet geometry on pathological files.** `compute_file_snippets` scans the
   source per symbol for anchors and line offsets. Files are capped at 8 MiB, but
   a generated file with tens of thousands of symbols is quadratic. A precomputed
   line index and identifier-anchor map would make it linear; deferred because no
   realistic corpus reproduced it.
4. **Full-index throughput.** Embedding dominates (42 s for 200 files / 2 000
   symbols here). Cross-file batching and model-level tuning need their own
   measured pass; parser and SQL changes cannot move this number materially.
5. **Lexical natural-language recall.** Identifier expansion helped, but lexical
   P@1 is still 0.750 and semantic P@1 0.550. Further gains require ranking work,
   not more feature surface, and must be validated on grounded tasks.
6. **Semantic R@5 regression.** The fusion now surfaces more lexical candidates;
   semantic R@5 fell 0.025 while R@10 rose 0.025. A future ranking change should
   recover rank-5 precision without sacrificing the overall gain.

---

## H. Final product assessment

Lynx is now a **focused, lightweight, high-performance repository discovery
engine** for the operations that dominate real use:

- Exact resolution and structural relations are deterministic and
  sub-millisecond warm.
- Lexical search is model-free, 8.8 ms cold and 0.14 ms warm, with improved
  identifier matching.
- MCP startup on an existing index is 11 MB and instant for structural/lexical
  clients; the model is loaded only when a semantic vector is requested.
- No-change and deletion-only indexing are effectively free on the measured
  200-file workspace (0.02 s / 0.05 s) and no longer load the model.
- Retrieval quality improved overall (P@1 0.793 → 0.815, R@10 0.956 → 0.981,
  MRR 0.893 → 0.911) with exact, ambiguity, relation, and structural behavior
  unchanged.
- The architecture remains small: index → retrieve → resolve → inspect →
  return evidence. No agent, planner, mutation path, cross-file graph, daemon, or
  network service was added.

The main unresolved cost is the embedding model itself, paid only when semantic
work is requested. That is a deliberate product boundary, now measured and
documented rather than hidden in every command's startup path.
