# Concepts

This page is the mental model behind the CLI and MCP surfaces. It describes what
Lynx produces, how it ranks, and where the guarantees end.

## What Lynx is

Lynx is a local, deterministic **repository intelligence** engine:

```text
source tree -> parse -> symbols + intra-file relations -> index
                                                        |
query -> lexical + semantic retrieval -> fuse -> rank -> evidence
```

It answers one class of question well:

> Where does X live in this repository, and what is it connected to?

Lynx is **not** an agent, planner, editor, or reasoner. It does not modify code,
decide what you should do, or explain architecture. It returns evidence and lets
the consumer decide.

## The index

`lx index` writes a persistent index at `<workspace>/.lynx`:

```text
.lynx/
├── graph.sqlite3    symbol identity, relations, vectors, provenance
└── lexical/         BM25 documents and snippets
```

The index is the unit of persistence. Retrieval commands hydrate it and never
walk the repository. `lx mcp` hydrates it too, building it once at startup only
when the workspace has never been indexed.

Index updates are incremental: a content-hash diff identifies added, modified,
and deleted files, and only those files are re-parsed and re-embedded. `--force`
discards the directory and rebuilds.

### What is excluded

The walk always skips `.git`, `.lynx`, `.fastembed_cache`, `node_modules`,
`vendor`, `target`, `build`, and `dist`. Test, mock, and generated paths are
skipped unless the index is built with `--include-tests`. Files over 8 MiB,
binary files, and files that fail to parse are skipped without failing the run.
Symlinks are not followed.

### Freshness

`lx status` reports one of:

| State | Meaning |
|---|---|
| `fresh` | The workspace content matches the indexed snapshot. |
| `stale` | The workspace changed since indexing. Queries still answer from the old index and warn on stderr. |
| `missing` | No index exists; run `lx index`. |
| `incompatible` | The index was produced by a different format; rebuild with `lx index --force`. |
| `corrupt` | The index cannot be read; rebuild with `lx index --force`. |
| `unknown` | The recorded workspace root is absent; freshness cannot be checked. |

Freshness is a convenience signal, not a transactional guarantee. Re-index after
pulling a branch, switching worktrees, or making substantial edits if you need
current coordinates.

## Symbols

A symbol is a named declaration: function, method, struct, class, trait,
interface, enum, module, variable, or constant. Every symbol has a stable
`SymbolIdentity`:

| Field | Meaning |
|---|---|
| `fqdn` | Fully qualified name, `::`-separated for path/module scopes. Python methods use a dot (`service::Service.run`). |
| `kind` | Structural role (`Function`, `Method`, `Struct`, …). |
| `language` | Source language (`Rust`, `Go`, `TypeScript`, …). |
| `file_path` | Workspace-relative path. |
| `content_hash` | Opaque producer hash over the file path, fqdn, and node text. Stable for an unchanged symbol; the address used by MCP `inspect`, `relations`, and `similar`. |

The module prefix is derived from the file path: the extension is stripped and
a leading `src/` is dropped (`src/auth/service.rs` → `auth::service`). Go
additionally uses the package clause. Examples:

```text
auth::AuthService::Login          # Go method
auth::service::AuthService::login # Rust method
service::Service.run              # Python method
web::login::Login                 # TypeScript function
```

`resolve` accepts either the full fqdn or the trailing name (`Login`). A bare
name can be ambiguous; `lx resolve` prints the deterministic winner and lists
the alternatives, while MCP `resolve_all` returns all of them.

## Evidence

Retrieval returns `Evidence`, not raw text chunks:

| Field | Meaning |
|---|---|
| `identity` | The symbol's `SymbolIdentity`. |
| `snapshot` | Provenance: commit hash (if Git), workspace root, dirty flag, and a working-tree content hash. |
| `range` | Inclusive line range and byte range in the file. |
| `code_snippet` | A bounded verbatim excerpt, at most 25 lines (80 for a module pseudo-symbol). |
| `score` | Channel- or fusion-assigned relevance score. |
| `retrieval_mode` | `Lexical`, `Semantic`, `Structural`, or `Hybrid`. |

The snapshot lets a consumer pin a finding to a tree state. The range lets it
open the exact span rather than re-searching the file.

## Retrieval modes

| Mode | What it does | Use it for |
|---|---|---|
| `lexical` | BM25 over the Tantivy index. | Exact words, identifiers, error strings. |
| `semantic` | Cosine similarity over embeddings of symbol text. | Paraphrase, concepts with little lexical overlap. |
| `structural` | Exact symbol lookup only; bypasses ranking. For natural-language input it returns no candidates. Use `resolve` and `relations` for the full structural surface. | Known names (`Login`). |
| `hybrid` | Fuses lexical and semantic channels, then applies deterministic adjustments. Default for `lx search`. | General "where is X" questions. |

Bare and qualified symbol queries take the deterministic route first: `Login`,
`auth.Login`, and `ValidateToken` are not turned into embedding searches.

### Ranking

Hybrid ranking works in this order:

1. Retrieve up to a bounded candidate set from each channel.
2. Fuse the two rankings with reciprocal rank fusion (RRF), a symmetric
   rank-based score.
3. Apply a `1.5×` boost to declaration kinds (functions, methods, types, and
   other definitions).
4. Demote non-production paths: test/mock/fixture by `0.5×`, vendored and
   third-party code by `0.25×`.
5. Normalize scores and sort.

Ties are broken deterministically by lexical rank, then semantic rank, then a
stable hash order. No tie-break depends on an opaque hash in a way that changes
with unrelated content.

`lx search --explain` exposes the two channel ranks, the raw RRF score, the
definition boost, and the final score for every result.

### Bounds

Every query is bounded:

- A per-channel candidate cap derived from `--limit` (clamped between 100 and
  2000, with an absolute caller maximum of 100,000).
- `--timeout-ms` sets a wall-clock deadline.
- The engine observes a cooperative cancellation token while streaming
  candidates.
- Per-query memory is proportional to the candidate cap, not the repository.
- Semantic retrieval is an exact scan over stored vectors; there is no
  approximate-nearest-neighbour index.

## Relations

Relations are directed edges with six kinds: `Calls`, `CalledBy`, `Implements`,
`ImplementedBy`, `Imports`, and `Contains`.

**Relations are intra-file only.** An edge is emitted only when both endpoints
were extracted from the same source file. Lynx does not link cross-file calls,
build a whole-repository call graph, or resolve symbols through imports. Treat
`relations` as local structure, not a complete call graph.

## Capability levels

Every index records the strongest guarantee it can make:

| Level | Meaning |
|---|---|
| `L0Text` | Raw text only. |
| `L1Parsed` | Syntax-aware chunks, no symbol resolution. |
| `L2Symbolized` | Symbol definitions extracted and identified. |
| `L3Structural` | Local relations available. |

Rust, Go, TypeScript, JavaScript, and Python adapters are grammar-backed and run
at `L3Structural`. Markdown, YAML, JSON, TOML, and unknown text use a
grammar-less adapter at `L1Parsed`; they produce file/module documents and no
relations. A workspace's capability is the maximum across its files.

## Context packages

`lx context` (and MCP `context`) assembles a `ContextPackage`:

```json
{
  "target_query": "token refresh",
  "snapshot": { "commit_hash": "…", "workspace_root": "…", "is_dirty": false, "content_hash": "…" },
  "primary_evidence": [ { "identity": { }, "range": { }, "code_snippet": "…", "score": 0.94, "retrieval_mode": "Hybrid" } ],
  "structural_relations": [ { "source_id": "…", "target_id": "…", "kind": "Calls" } ],
  "estimated_tokens": 1874,
  "capability_level": "L3Structural"
}
```

Token estimation uses **4 characters ≈ 1 token**. Envelope overhead (query,
per-evidence identities, relations) is reserved first; the remaining budget is
distributed across snippets in rank order, truncating at whole-line boundaries
(with a bounded character fallback when a single line exceeds the budget).
`estimated_tokens` is within the requested budget.

Context ordering inherits retrieval ordering. The right symbol is usually in
the package, but not guaranteed to be first.

## Determinism

For the same index and query, structural lookups (`resolve`, `relations`) return
the same answer. Hybrid ranking is deterministic given the same channel results
and a stable tie-break. Semantic ranking depends on the embedding model and its
runtime; treat it as reproducible on the same build and platform, not as a
mathematical guarantee across model or ONNX Runtime versions.

## Deliberate limits

These are product boundaries, not bugs:

- Relations are intra-file.
- Semantic top-1 is representation-limited: recall is strong, but the first
  result is not always the intended one. Lexical and structural lookups are
  precise.
- The index stores snippets, not a full AST or type information.
- There is no cross-repository search.
- There is no daemon, network service, or filesystem watcher. You re-index
  explicitly.
- Very large repositories are supported up to the measured envelope, but
  semantic search is an exact scan.

See [troubleshooting](troubleshooting.md) when a query behaves unexpectedly, and
the [final product report](architecture/final-product-report.md) for measured
quality numbers.
