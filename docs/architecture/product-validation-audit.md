# Lynx Product Validation Audit

Scope: the `crates/*` workspace at the start of this validation pass
(commit `0064c37`, branch `optimization/func`), evaluated as an **independent
repository-intelligence engine** rather than a PizenLabs component.

Method: read the full call graph, then measure the production paths with the
harness in `benchmarks/`. Every claim below is backed by source or a measured
number; measurements were taken on the development machine (Apple silicon,
release build). Numbers labelled **before** were captured before any change in
this pass; numbers labelled **after** were captured with the remediation in
[`product-validation-report.md`](product-validation-report.md).

---

## 1. Executive summary of the audit

Lynx's **retrieval primitives** (exact resolution, lexical+sematic fusion,
relations, context compilation) were already correct, bounded, cancellable, and
deterministic. Its **persistence and lifecycle** were not: the engine had no
hydration path, so *every* retrieval invocation re-indexed the whole repository
in-process, and repository-proportional state (every snippet, every vector) was
retained in RAM for the life of the process.

The single largest defect was therefore not a ranking or memory bug inside a
query — it was that the product's own persistence artifact was write-only. This
audit found and fixed that, plus two correctness bugs that the fixed lifecycle
surfaced.

---

## 2. Ownership map

Each row is a responsibility with its pre-change owner, contract, and resource
behaviour. "Pre-change" = commit `0064c37`.

| Responsibility | Owner (pre-change) | Inputs | Outputs | State | Lifetime | Failure behaviour | Resource behaviour |
|---|---|---|---|---|---|---|---|
| Repository discovery | `lynx-core::engine::index_repository` (`WalkDir`) | repo path | file list | none | per call | per-file `continue` on read/parse error | walks the whole tree per call |
| Parsing | `lynx-parser::Parser` / `adapter_for_path` | file path, source | `Vec<SymbolIdentity>`, `Vec<Relation>` | none (stateless) | per file | `Err(ParseError)` → file skipped | builds a `Box<dyn LanguageAdapter>` per file; tree dropped per file |
| Symbol extraction | language `*Adapter` | tree + source | `RawSymbol` | none | per file | empty result on no match | O(file) |
| Symbol identity | `LanguageAdapter::parse_symbols` | source | `SymbolIdentity` | none | per file | empty fqdn rejected | hash over source text (**F3**) |
| Snippet extraction | `engine::compute_file_snippets` | source, identities | `(SourceRange, String)` | none | per file | clamps to `MAX_SNIPPET_LINES` | O(file) |
| Lexical indexing | `lynx-storage::TantivyStore` | `LexicalDoc`s | index segments | shared `IndexWriter` | per index pass | `Err(StorageError)` | 15 MB writer heap; one commit per pass |
| Semantic indexing | `lynx-embed::FastEmbedProvider` | batch of texts | vectors | `Mutex<TextEmbedding>` | process | `Err(EmbedError)` | ~250 MB ONNX model, once per process |
| Vector storage | `Engine.embeddings: HashMap` | vectors | — | **in memory only** | process (**F2**) | — | O(symbols × dim) retained |
| Metadata storage | `GraphStore` (SQLite) | identities, ranges, relations | rows | WAL DB | disk | `Err(StorageError)` | bounded |
| Persistence | `lynx-mcp::Session::persist` | staging dir | `./.lynx` | disk | until re-index | `Err` | copies/renames a tree |
| Hydration | **none** | — | — | — | — | — | **F1** |
| Search | `Engine::search_with` | query, mode, limit | `Vec<Evidence>` | locks `symbols`+`embeddings` | per query | bounded | O(candidates) transient |
| Ranking / fusion | `lynx-core::retrieval` | channel ranks | fused scores | none | per query | deterministic | O(candidates) |
| Exact resolution | `Engine::resolve_index` | name | hash | `HashMap` in RAM | process (**F2**) | `NotIndexed` | O(symbols) map |
| Relations | `GraphStore::get_relations` | hash, kind | edges | SQLite | disk | `Err` | bounded |
| Context construction | `lynx-core::compiler` | evidence, relations, budget | `ContextPackage` | none | per query | never exceeds budget | bounded |
| Cancellation | `lynx-core::query::CancelToken` | flag | — | `Arc<AtomicBool>` | per query | observed per candidate | O(1) |
| Timeout | `QueryOptions::deadline` | instant | — | per query | per query | `TimedOut` | O(1) |
| Resource limits | `engine::candidate_limit` | limit | candidate cap | none | per query | clamps | O(candidates) |
| MCP lifecycle | `lynx-mcp::rpc::serve` | stdin | stdout frames | one `Engine` | process | malformed frame contained | serial, blocking |
| CLI lifecycle | `lynx-cli::main` | argv | stdout | one `Session` | process | `anyhow` message | one process per invocation |

---

## 3. Actual call graphs (pre-change)

Traced from code, not filenames.

```text
lx index <path>
  storage = "./.lynx" (cwd-relative)
  remove storage if --force
  Session::open(root, include_tests)          // lynx-mcp::session
    FastEmbedProvider::with_cache_dir         // loads ONNX (~250 MB)
    tempfile::tempdir()                       // ephemeral substrate
    Engine::new(temp, provider)
    Engine::index_repository(root)            // FULL PASS
      WalkDir -> read_to_string -> Parser::parse
      per file: GraphStore::insert_symbols, insert_relations
                TantivyStore::add_documents   // shared writer
                provider.embed_batch
                symbols.insert(...)           // retained in RAM forever
                embeddings.insert(...)        // retained in RAM forever
    Engine::search? no
  Session::persist("./.lynx")                 // move/copy temp -> ./.lynx

lx search <q>
  open_session()
    workspace_root() = git toplevel(cwd) or cwd
    Session::open(root, false)                // <-- RE-INDEXES THE WHOLE REPO
  Engine::search_with(q, mode, limit)
    resolve_index lookup (O(1))
    TantivyStore::search_snippets
    scan ALL embeddings in RAM -> top-k
    rrf_fuse -> assemble evidence (O(candidates))
    prepare/boost/finalize/truncate
```

`lx resolve`, `lx relations`, `lx context` share the same `open_session()` and
therefore each **re-index the repository** before answering. `lx mcp` calls
`Session::open` once at startup (so it indexes once per server process, not per
query — this is why the MCP RSS probe looked flat pre-change).

MCP request path:

```text
stdin line
  rpc::handle_line -> serde_json parse (malformed -> -32700, contained)
  rpc::route
    "initialize" | "ping" | "tools/list"
    "tools/call" -> tools::dispatch -> Engine primitive -> to_value -> envelope
  writeln!(stdout)
```

---

## 4. Persistence lifecycle (pre-change)

The required lifecycle is:

```text
index -> persist -> process exit -> new process -> hydrate -> query
```

What actually happened:

```text
index -> persist (write-only artifact)
process exit
new process
(re)index the entire repository      <-- hydration does not exist
query
```

`Session::open` unconditionally built a fresh ephemeral substrate and ran a full
index pass. `GraphStore` and `TantivyStore` were never read back into an engine;
`DualStorage` exposed only write paths used by retrieval. The `.lynx` artifact
produced by `lx index` was consumed by nothing.

**Consequence:** cold-query latency and RSS were those of a full indexing pass.
Measured `lx search "func_5_3"` on a 100-file/1 100-symbol synthetic repo:
**8.63 s, 315 MB** (the same repo's `lx index` was 10.44 s, 313 MB).

---

## 5. Resource lifecycle

| Resource | Created | Owner | Scope | Released | Max lifetime |
|---|---|---|---|---|---|
| ONNX `TextEmbedding` | provider ctor | `Engine.provider` | process | `Engine` drop | process |
| Tantivy `Index` + `IndexWriter` | `TantivyStore` | store | process / pass | drop / commit | process |
| Tantivy `IndexReader` | `search_snippets` | **per query** | query | call end | query |
| Parser tree | `Parser::parse` | per file | file | call end | file |
| `StoredSymbol` map (snippets) | `index_repository` | `Engine.symbols` | index | `Engine` drop | **process** |
| `embeddings` map (vectors) | `index_repository` | `Engine.embeddings` | index | `Engine` drop | **process** |
| Fused candidates | `search_with` | query | query | call end | query |
| `Evidence` (pre-truncation) | `search_with` | query | query | call end | query |
| SQLite connection | `GraphStore` | store | process | drop | process |
| Query candidates | `search_with` | query | query | call end | query |

The two bold rows are the ones that made memory proportional to repository size.

---

## 6. Findings

Severity: P0 = correctness/corruption/lifecycle; P1 = retrieval/perf/reliability.

### F1 — No hydration; every retrieval invocation re-indexes the repository (**P0**)
`lynx-mcp::session::Session::open` always created an ephemeral substrate and ran
`Engine::index_repository`. `Engine` had no load path. Measured: `lx search`
8.63 s / 315 MB on a repo whose `.lynx` already existed. Violates §2.3, §14, §23.

### F2 — Repository-proportional state retained in RAM (**P0**)
`Engine.symbols: HashMap<String, StoredSymbol>` held every snippet; `Engine.embeddings: HashMap<String, Vec<f32>>` held every vector, for the process
lifetime. Indexing RSS grew with symbol count (313 MB → 358 MB from 1 100 →
8 400 symbols, on a ~250 MB ONNX baseline) and never returned. Violates §15, §23.

### F3 — Identical source text collides under the `content_hash` primary key (**P0, correctness/data loss**)
`adapter.rs` hashed only the source text (`hash_text(slice)`), and the store's
`symbols.content_hash` is a `PRIMARY KEY` with `INSERT OR IGNORE`. Two distinct
symbols whose bodies are byte-identical (e.g. `func Login` in `package auth` and
`package admin`) produced the same hash, so one was **silently dropped**.
Demonstrated by the ambiguity corpus before the fix (`resolve_all("Login")`
returned 2 of 3 symbols).

### F4 — Persistence was one-directional (write-only) (**P0, dup of F1**)
`Session::persist` moved the staging directory into `./.lynx`; no reader existed.
The `hardening-report.md` already flagged this as "the largest remaining product
gap".

### F5 — Exact resolution hid ambiguity (**P1**)
`resolve` returned a single deterministic winner (`ResolveIndex.by_name`) with no
way to observe that a trailing name was shared. `resolve("Login")` returned one
of several real `Login`s; a consumer could not tell. Violates §12.

### F6 — Malformed lexical queries failed the whole query (**P1, reliability**)
`TantivyStore::search_snippets` propagated `QueryParserError`, so a query Tantivy
could not parse (e.g. `auth::Middleware::Authenticate`, `""`) aborted the
retrieval with `storage failure: lexical query parse failure`. A caller's query
syntax must not fail the query. Violates §17, §25.

### F7 — Indexing memory not bounded by repository size (**P1**)
Even after the earlier hardening, `index_repository` accumulated `symbols` and
`embeddings` maps before storing them at the end of the pass, so peak RSS scaled
with repository size (measured above). Violates §15, §16.

### F8 — `search_snippets` built a fresh `IndexReader` per query (**P2**)
Source-confirmed; a query paid reader construction on every call.

### F9 — Conceptual/context ranking is materially weaker than exact/lexical (**P1**)
Pre-change benchmark (10 queries): real-embedding Precision@1 0.700,
Recall@5 0.883. The expanded suite (135 tasks) shows context P@1 0.100 while
Hit@5 0.800 — the right symbol is usually *in* the package but rarely first.

### F10 — No incremental indexing (**P1**)
No file-level state; re-index is all-or-nothing. Re-indexing into an existing
substrate relied on `INSERT OR IGNORE` + append-only Tantivy, which cannot remove
stale rows (addressed in this pass by a full reset before each pass).

### F11 — No staleness signal on the query path (**P2**)
A hydrated index cannot tell a consumer the workspace changed since indexing.
`IndexStatus.snapshot` carries `commit_hash`/`is_dirty`/`content_hash`, but the
query paths did not compare them.

### F12 — Error taxonomy is coarse at the transport boundary (**P2**)
`CoreError` distinguishes storage/parse/embed/io/git/not-indexed/dimension/
cancel/timeout, but there is no `InvalidQuery`, `IndexCorrupt`, `ResourceLimit`,
or `UnsupportedRepository` distinction; MCP maps query errors to `-32603`.

---

## 7. What the remediation changed (summary)

| Finding | Fix | Layer |
|---|---|---|
| F1, F4 | `Engine::new` hydrates; retrieval opens the persisted index; MCP hydrates (or indexes once at startup) | ownership / lifecycle |
| F2, F7 | `Engine` retains no symbols/vectors; queries stream from SQLite; indexing streams per file | data model / resource boundary |
| F3 | `content_hash = blake3(fqdn ‖ text)` | symbol identity |
| F5 | `Engine::resolve_all` + MCP `resolve_all` + CLI alternatives; noise-aware deterministic ordering | retrieval |
| F6 | malformed/empty lexical queries degrade to no lexical candidates | failure containment |
| F8 | one cached `IndexReader`, reloaded on commit | resource lifecycle |
| F10 | `DualStorage::reset` clears graph + lexical before each index pass | persistence |
| F9 | benchmarked, not weight-tuned (see report) | measurement |

Full before/after numbers and the capability matrix are in
[`product-validation-report.md`](product-validation-report.md) and
[`production-readiness.md`](production-readiness.md).
