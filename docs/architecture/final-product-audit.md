# Lynx — Final Product Architecture Audit

Scope: the `crates/*` workspace as of the final productization pass (branch
`optimization/func`). This document describes the product as it **is** — one
coherent engine — so a new contributor can understand it without reconstructing
the repository mentally. It supersedes the pre-change audits
([`product-validation-audit.md`](product-validation-audit.md),
[`production-audit.md`](production-audit.md)) for the current state.

Lynx is a **deterministic repository intelligence engine for AI-native developer
tooling**. It **discovers**; consumers **decide**.

---

## 1. The shape of the product

```text
                 Consumer (agent, IDE, CLI, review bot, automation)
                                     |
                                     v
                                MCP  /  CLI
                                     |
                                     v
        +------------------------------------------------+
        |                     Lynx                        |
        |                                                 |
        |  index  ->  persist  ->  hydrate  ->  query     |
        |                                       |         |
        |     discovery / parse / symbol identity          |
        |     retrieval / ranking / resolution             |
        |     relations / context compilation              |
        |     resource bounds / cancellation / timeouts    |
        +------------------------------------------------+
                                     |
                                     v
                              Repository truth
```

Lynx owns discovery, parsing, symbol identity, relationships, indexing,
persistence, retrieval, ranking, resolution, context construction, resource
control, explainability, and MCP/CLI exposure. It does **not** own agent
reasoning, planning, execution, authorization, or code modification.

Seven crates, one direction of dependency:

```text
lynx-protocol   frozen contracts (no I/O, no business logic)
lynx-parser     source -> SymbolIdentity + intra-file Relation
lynx-embed      text -> dense vector (VectorProvider seam)
lynx-storage    DualStorage: SQLite (identity/graph/vectors) + Tantivy (lexical)
lynx-core       Engine: the indexing + retrieval pipeline (owns the product logic)
lynx-mcp        stdio JSON-RPC transport + Session lifecycle
lynx-cli        `lx` command-line transport
```

`lynx-protocol` is the stability boundary: every type a consumer sees lives
there, and any change to a serialized form is a breaking change. Nothing below
`lynx-core` knows about consumers; nothing above `lynx-core` re-implements
retrieval.

---

## 2. Ownership

| Responsibility | Owner | Notes |
|---|---|---|
| Repository discovery | `lynx-core::engine::scan_workspace` | Walks the tree, applies the exclusion + size + UTF-8 predicate, returns `(path, content_hash)` pairs only. |
| Parsing | `lynx-parser::Parser` → `adapter_for_path` | Stateless; one `Box<dyn LanguageAdapter>` per file. |
| Symbol extraction | language `*Adapter` | Tree-sitter queries for Rust/Go/TS/JS/Python; `GenericAdapter` for everything else. |
| Symbol identity | `lynx-parser::adapter::hash_identity` | `blake3(path ‖ fqdn ‖ text)` — unique per `(file, symbol)`. |
| Snippet extraction | `lynx-core::engine::compute_file_snippets` | Symbol-anchored, non-overlapping, bounded (`MAX_SNIPPET_LINES`). |
| Incremental diff | `lynx-core::engine::Engine::diff_plan` | Diffs current file hashes against persisted `FileState`s. |
| Indexing pipeline | `lynx-core::engine::Engine::index_repository` / `update_repository` | Streaming; `O(largest file + batch)`. |
| Persistence | `lynx-storage::DualStorage` | `GraphStore` (SQLite WAL) + `TantivyStore`. |
| Hydration | `lynx-core::engine::Engine::new` → `hydrate` | Reads provenance + counts only; never re-reads source. |
| Lexical retrieval | `lynx-storage::TantivyStore::search_snippets` | BM25 over stored snippets; malformed queries degrade to empty. |
| Semantic retrieval | `lynx-core::engine::Engine::search_with` + `GraphStore::for_each_embedding` | Streams vectors; bounded `TopK`. |
| Ranking / fusion | `lynx-core::retrieval` | RRF + definition boost + noise penalty; deterministic tie-breaks. |
| Exact resolution | `lynx-core::engine::Engine::resolve` / `resolve_all` | Bounded SQL; production-before-noise; lexicographic ties. |
| Relations | `lynx-core::engine::Engine::relations` / `trace` | Intra-file graph; BFS trace bounded by depth + deadline. |
| Context construction | `lynx-core::compiler::ContextCompiler` | Token-budgeted, line-boundary preserving. |
| Cancellation | `lynx-core::query::CancelToken` | `Arc<AtomicBool>`, observed per candidate. |
| Timeouts | `lynx-core::query::QueryOptions::deadline` | Wall-clock; observed at stage boundaries and per candidate. |
| Resource limits | `lynx-core::engine` constants + `GraphStore::PARAM_CHUNK` + `TantivyStore::WRITER_HEAP_BYTES` | See §5. |
| Freshness | `lynx-core::engine::Engine::freshness` | Explicit re-scan; `Fresh/Stale/Missing/Incompatible/Corrupt/Unknown`. |
| MCP | `lynx-mcp::rpc` + `lynx-mcp::tools` + `lynx-mcp::session` | Transport, tool mapping, lifecycle. |
| CLI | `lynx-cli::main` | Transport only; no retrieval logic. |

---

## 3. Lifecycle

```text
lx index <repo>            # incremental by default; --force = full rebuild
  resolve_snapshot         # git HEAD, dirty flag, tree content hash
  scan_workspace           # (path, content_hash) for each indexable file
  diff_plan                # added / modified / deleted / unchanged
  delete stale rows+docs   # SQLite rows and Tantivy docs for deleted+modified
  index_file (changed set) # parse -> symbols -> relations -> snippets -> vectors
  upsert_files             # persist per-file content hashes
  commit                   # one Tantivy commit per pass; index quiescent
  set_meta                 # latest snapshot, capability, include_tests
        |
        v
  <repo>/.lynx/            # graph.sqlite3 (WAL) + lexical/ (Tantivy)
        |
        v
  process exits
        |
        v
lx search|resolve|relations|context|status      # new process
  Engine::new              # HYDRATE: provenance + counts; no source read
  query                    # answered from SQLite + Tantivy; never re-indexes
        |
        v
  update / re-index        # `lx index` again; only diffs are touched
```

`lx mcp` hydrates at startup and builds once only if the index is absent; it
reuses that one engine (and one embedding provider) for the process lifetime.

### 3.1 The embedding model is a process singleton

There is exactly **one** `VectorProvider` per process, created once and shared
by indexing and querying. This is load-bearing: constructing a second model
instance in the same process changes inference results from a fresh process and
must not be done. The MCP server, the CLI, and the validation harness all follow
this rule.

---

## 4. State

| Class | State | Where | Lifetime |
|---|---|---|---|
| Persistent | identities, ranges, relations, vectors, file hashes, snapshots, meta | `graph.sqlite3` | until `lx index --force` or schema change |
| Persistent | lexical snippet documents | `lexical/` (Tantivy) | committed per pass |
| Derived | lexical index, embedding vectors | storage | rebuilt from source + model |
| Ephemeral (per query) | candidate lists, `TopK`, fused scores, `Evidence`, `QueryReport` | query stack | released at return |
| Cache | prepared SQLite statements, Tantivy `IndexReader`, ONNX session | engine | process |
| Cache (disk) | embedding-model weights | `~/.cache/lynx/fastembed` | user cache; never inside a workspace |

Nothing repository-proportional is retained in RAM: hydrated state is counts and
provenance only, and every query materializes at most its candidate bound.

---

## 5. Resource boundaries

| Allocation | Owner | Lifetime | Upper bound | Released |
|---|---|---|---|---|
| Source file text | `index_file` | per file | `MAX_INDEX_FILE_BYTES` (8 MiB); larger files skipped | end of file |
| Snippet text | `compute_file_snippets` | per file | `MAX_SNIPPET_LINES` (25) / module 80 lines | after storage write |
| Embedding batch | `index_file` | per file | one file's symbols, each `<= MAX_EMBED_CHARS` (512) | after write |
| Tantivy writer heap | `TantivyStore` | per pass | `WRITER_HEAP_BYTES` (15 MB) | `commit`/drop |
| Semantic candidates | `TopK` | per query | candidate cap (100–2000, hard max 100 000) | query end |
| `Evidence` retained | `assemble`/`materialize_ranked` | per query | `limit` | query end |
| SQLite binds | `GraphStore::symbols_for` | per call | `PARAM_CHUNK` (500) per statement | call end |
| ONNX session | `FastEmbedProvider` | process | ~250 MB resident (model) | process exit |
| Trace frontier | `Engine::trace_with` | per query | depth-bounded BFS; deadline-checked | query end |

A normal query never allocates proportionally to repository size. Large
repositories affect **cost** (`O(N)` semantic scan), not retained memory.

---

## 6. Failure boundaries

```text
parser      per-file ParseError  -> file skipped, indexing continues (never fatal)
embedding   EmbedError           -> propagates out of index_file -> pass fails loudly
storage     StorageError         -> propagates -> open/query reports the fault
retrieval   bounded; Cancelled/TimedOut are explicit terminal errors
MCP         malformed frame -> -32700; bad params -> -32602;
            primitive failure -> isError=true; server survives every case
CLI         anyhow error + non-zero exit; no panic on malformed input
```

Classification produced by the engine:

| Condition | Signal |
|---|---|
| Not indexed | `CoreError::NotIndexed` (MCP `index_status` reports `freshness`) |
| Incompatible schema | `Freshness::Incompatible` (substrate was reset on open) |
| Corrupt artifact | `Freshness::Corrupt` (`PRAGMA quick_check`) |
| Stale index | `Freshness::Stale` (`Engine::freshness`) |
| Cancelled | `CoreError::Cancelled` |
| Timed out | `CoreError::TimedOut` |
| Ambiguous name | not an error: `resolve_all` returns every candidate |
| Invalid query | rejected at the transport boundary; malformed lexical degrades to no lexical hits |

---

## 7. Determinism

- Exact/structural resolution is a bounded SQL lookup with a total order:
  exact-fqdn first, production before test/vendor, then lexicographic
  `(fqdn, content_hash)`.
- Fusion (`rrf_fuse`) has a total order: score desc, then lexical rank, then
  semantic rank, then hash. RRF is symmetric, so ties are inherent; resolving
  them by lexical rank both removes dependence on opaque hashes and matches the
  high-precision channel. `finalize_fused` uses a **stable** sort so it never
  disturbs that order.
- The same repository state and query produce the same result across repeated
  calls, process restarts, hydration, and re-index.

---

## 8. What is deliberately *not* here

- No ANN/approximate vector index: at the measured envelope the exact scan is
  sub-millisecond and an ANN would add complexity without product benefit.
- No distributed components, no background workers, no per-query tasks.
- No cross-file relations (relations are intra-file; see limitations).
- No agent loop, planning, or code modification.
