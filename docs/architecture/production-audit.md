# Lynx Production Audit (Phase 0–1)

Scope: the `crates/*` workspace at commit `e54c768` (branch `optimization/func`).
Method: read the full call graph, then measure with a synthetic multi-language
repository generator (`benchmarks/scripts/gen_repo.py`) and a stage profiler
(`crates/lynx-core/examples/profile_index.rs`).

Nothing below is asserted without either source evidence or a measured number.

---

## A. Architecture map

```text
CLI: pizen-lynx (bin `lx`)            crates/lynx-cli/src/main.rs
  index     -> lynx_mcp::Session::open(root)  (full index in a temp dir) -> persist(./.lynx)
  search    -> open_session() -> Session::open (re-index the whole workspace)
  resolve   -> open_session() -> Session::open (re-index)
  relations -> open_session() -> Session::open (re-index)
  context   -> open_session() -> Session::open (re-index)
  mcp       -> lynx_mcp::run -> Session::open once -> rpc::serve(stdin loop)

MCP: pizen-lynx-mcp                    crates/lynx-mcp
  Session { engine: Engine, workspace_root, _substrate: tempfile::TempDir }
  rpc::serve   serial, blocking line loop; one Engine for the process lifetime
  tools        dispatch -> Engine privitives

CORE: pizen-lynx-core                  crates/lynx-core
  Engine {
    dual:       DualStorage { graph: GraphStore(SQLite), lexical: TantivyStore(Index) }
    parser:     Parser (stateless; builds a Box<dyn LanguageAdapter> per file)
    provider:   Box<dyn VectorProvider>  (FastEmbedProvider { Mutex<TextEmbedding> })
    snapshot:   Mutex<Snapshot>
    symbols:    Mutex<HashMap<String, StoredSymbol>>   <-- every snippet retained
    embeddings: Mutex<HashMap<String, Vec<f32>>>       <-- every vector retained
    status:     Mutex<IndexStatus>
  }

STORAGE: pizen-lynx-storage
  GraphStore:  Mutex<rusqlite::Connection> (bundled SQLite, WAL, synchronous=NORMAL)
  TantivyStore: tantivy Index (MmapDirectory) + 4 fields

PARSER: pizen-lynx-parser               tree-sitter per extension; intra-file relations only
EMBED:  pizen-lynx-embed                fastembed bge-small-en-v1.5, 384-dim, Mutex-guarded
PROTOCOL: pizen-lynx-protocol           pure serde DTOs (frozen wire forms)
```

## B. Resource ownership map

| Heavyweight | Created by | Owner | Borrowers | Cloned | Cached | Dropped | Background lifecycle |
|---|---|---|---|---|---|---|---|
| ONNX `TextEmbedding` (~250 MB) | `FastEmbedProvider::with_cache_dir` | `Box<dyn VectorProvider>` in `Engine` | `embed_query`/`embed_batch` (Mutex) | no | no | `Engine` drop | ONNX runtime worker threads |
| tantivy `Index` | `TantivyStore::open` | `TantivyStore` | search/writer | cheap (`Arc`) | no | `DualStorage` drop | — |
| tantivy `IndexWriter` | **`add_documents`, once per file** | local | — | — | no | end of `add_documents` | merge threads spawn per writer |
| tantivy `IndexReader` | **`search_snippets`, once per query** | local | — | — | no | end of call | — |
| `StoredSymbol` map | `index_repository` | `Engine.symbols` | search/resolve/similar | `to_evidence` clones identity + range + snippet | **forever** | `Engine` drop | — |
| `embeddings` map | `index_repository` | `Engine.embeddings` | search/similar | hash `String` cloned per candidate | **forever** | `Engine` drop | — |
| temp substrate dir | `Session::open` | `Session._substrate` | engine | — | — | `Session` drop / `persist` | — |
| `Snapshot` | `resolve_snapshot` | `Engine.snapshot` | cloned **per evidence item** | yes | — | — | — |

Answers to "who keeps it alive": the `Engine` is the retention root. Everything
heavy (symbols, embeddings, the ONNX model, the SQLite connection) lives exactly
as long as the `Engine`; nothing is released early, and nothing is ever evicted.

## C. Search call graph (actual runtime path)

```text
lx search Q [--limit L] [--mode M]
  open_session()
    workspace_root() = git toplevel(cwd) or cwd
    Session::open(root, false)
      FastEmbedProvider::with_cache_dir(~/.cache/lynx/fastembed)   <-- loads ONNX
      tempfile::tempdir()
      Engine::new(temp, provider)
      Engine::index_repository(root, false)                        <-- FULL WORKSPACE PASS
        WalkDir every file; read_to_string; Parser::parse
        per file: graph insert (BEGIN/INSERT/COMMIT x2); lexical add_documents
                  (NEW 15 MB IndexWriter + commit PER FILE); embed_batch(snippets)
  Engine::search(Q, M, L)
    lock(symbols); lock(embeddings)                                <-- held for whole call
    lexical: TantivyStore::search_snippets(Q, max(L,50))  [new reader]
    semantic: provider.embed_query(Q)
              for EVERY embedding: cosine -> Vec<(String,f32)>     <-- clones every hash
              sort O(N log N)
    rrf_fuse(lexical, semantic) -> HashMap<String,f32> over union  <-- up to N entries
    for every fused entry: StoredSymbol::to_evidence(...)          <-- clones Snapshot + snippet + identity
    apply_definition_boost; finalize_fused (sort all); truncate(L)
```

Cardinalities: N = symbols, C = lexical hits (`max(L,50)`), D = 384.
Semantic produces an N-length candidate list; fusion and evidence materialization
are O(N) before the final `truncate(L)`.

## D. Memory-growth hypotheses, ranked by evidence

1. **Per-file `IndexWriter` create+commit in `add_documents`** — CONFIRMED dominant.
   Profiler: lexical stage = 24.996 s of 25.467 s (98%) on a 200-file repo.
   0.125 s/file of pure writer churn. A fresh 15 MB writer per file plus a commit
   (segment publish + merge-policy evaluation) per file, and the segment files are
   mmap'd by `MmapDirectory` (page cache shows up in RSS).
2. **`Engine::search` materializes `Evidence` for the entire candidate set before
   `truncate(L)`** — CONFIRMED by source. Every fused candidate clones a full
   `Snapshot` (2 `String` + `PathBuf`), the `SymbolIdentity` (`String`+`PathBuf`),
   and the whole `code_snippet` (module snippets are the *entire file*). Per-query
   allocation is O(N × snippet size); unbounded and not cancellable. Repeated in a
   long-lived MCP process it fragments the allocator and drives RSS up.
3. **Every retrieval CLI invocation re-indexes the whole workspace** — CONFIRMED.
   `lx search` inside a 2200-symbol repo: 115 s, 577 MB max RSS, for one query.
   `Session` cannot hydrate from `.lynx`; `Engine` has no load path.
4. **Unbounded in-memory retention** — CONFIRMED by source. `symbols` and
   `embeddings` are never evicted; module pseudo-symbols store the whole file text
   and embed the whole file text. Memory ≈ O(Σ source bytes) + O(N × D).
5. **No cancellation, timeout, or candidate/output bounds anywhere** — CONFIRMED by
   source. `search`, `similar`, `relations`, `trace`, `context` run to completion.
   This is why a pathological query "keeps running ... RSS grows beyond 15 GB if
   the process is not stopped."
6. **`resolve` is O(N) linear scan**, not the documented O(1) — CONFIRMED by source
   (`symbols.values().find(...)` twice). Determinism is fine; cost is not.

Not a hypothesis: allocator fragmentation alone, mmap alone, or ONNX alone. Each
was measured or ruled out (see §F).

## E. Retrieval failure analysis

- Exact symbol queries work and are deterministic (structural `resolve` bypasses
  ranking) — good.
- Conceptual queries are weak: the pipeline is a naive BM25⊕cosine RRF with a flat
  1.5× definition boost. There is **no** query classification, no qualified-name
  match, no path/symbol-kind signal, no test/generated/vendor penalty in ranking
  (those paths are skipped at *index* time only), and no deduplication. The RRF is
  unweighted (`1/(60+rank)` each channel) despite the spec's dynamic α/β.
- Module pseudo-symbols (whole-file, generic fqdn like `pkg5::mod_5`) are indexed
  and ranked like real code, adding noise.
- There is no explainability surface, so ranking cannot be diagnosed.

## F. What was measured

| Workload | Result |
|---|---|
| `lx index`, 50/100/200/400 files × 10 symbols | 28/58/111/222 s — **0.55 s/file, exactly linear** |
| max RSS, same | 541/552/550/572 MB — flat (ONNX ~250 MB + fixed) |
| stage profile, 200 files (fake 384-d provider) | read 0.04 s, parse 0.15 s, graph 0.28 s, **lex 25.0 s**, embed 0.002 s |
| `lx search "func_5_3"` inside 2200-symbol repo | **115 s, 577 MB** — full re-index per query |

Conclusion: the dominant pathologies are (1) per-file lexical writer churn,
(2) eager full-candidate evidence materialization in `search`, (3) whole-repo
re-index per retrieval invocation, and (4) total absence of lifetime bounds. RSS is
linear in symbols once #2 is exercised; there is no upper bound and no way to stop it.

## G. Production gaps

Runtime: per-file writer churn; O(N) per-query materialization; no cancellation or
timeout; no cache/retention strategy; resolve O(N).
Retrieval: no benchmark, no explainability, no noise penalties at rank time.
Observability: none beyond a single eprintln at MCP start.
Reliability: unbounded work on malformed/oversized inputs; failures are `continue`
during indexing but unbounded during query.

---

## Implementation plan (independent, verifiable slices)

1. **Storage:** one reusable `IndexWriter` + explicit `commit()`; batch commits.
2. **Core search:** bound the candidate set before materialization; avoid cloning
   `Snapshot`/snippet for discarded candidates; deterministic tie-breaks.
3. **Core resolve:** O(1) exact map (fqdn + trailing name), deterministic.
4. **Lifetime bounds & cancellation:** query deadline + `CancelToken` threaded
   through search/similar/context; explicit candidate/result caps.
5. **Observability:** structured per-query diagnostics; `--explain` ranking output.
6. **Retrieval benchmark:** corpus + ground truth + Precision@K/MRR harness.
7. **Regression tests** for every fixed failure mode.
8. **Stress + production contract** documentation.

Each slice ends with `cargo fmt --check`, `cargo check --workspace`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
