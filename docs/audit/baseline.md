# Lynx baseline — 2026-10-30

This document records the pre-optimization state of the repository at
`b87588b` (`perf/optimize-logic` branch) so every later change has a
reproducible reference. Raw harness output is in
`benchmarks/reports/baseline_20261030_154835.md` (generated, not committed).

## Environment

| Item | Value |
|---|---|
| OS | macOS (Darwin 27.0.0, arm64) |
| CPU | Apple Silicon (arm64) reference machine |
| Rust | rustc 1.99.1 (Homebrew), edition 2021 |
| Build | `--release`, `opt-level=3`, `lto=true`, `codegen-units=1`, `strip=true` |
| Corpus | `benchmarks/corpus/auth-service` (10 files, 47 symbols, 44 relations) |
| Synthetic corpus | `gen_repo.py`, Rust, 10 symbols per file |
| Embedder | FastEmbed `bge-small-en-v1.5`, 384 dimensions |
| Binary | `target/release/lx` @ `b87588b` |

> **Environment note.** These numbers are materially slower than the archived
> October 16 report on the same machine (for example, a 200-file full index was
> 16.5 s then and 42.5 s now; ONNX model load was ~300 MB peak then and
> ~1.5 GB now). The cause was not isolated to Lynx; the OS, SDK, and ONNX
> Runtime build changed between runs. All before/after comparisons in this audit
> use the same day, machine, build profile, and harness, so they remain
> internally valid. Absolute values are not comparable with older reports.

## Baseline measurements (measured, not estimated)

### Cold CLI operations (fresh process, 20 samples)

| Operation | p50 | p95 | Peak RSS |
|---|---:|---:|---:|
| `lx --version` | 6.7 ms | 7.0 ms | — |
| `lx resolve ValidateToken` | 13.5 ms | 13.9 ms | — |
| `lx search token --mode lexical` | 766 ms | 859 ms | — |
| `lx search "token validation"` (hybrid) | 823 ms | 823 ms | 603 MB |
| `lx status --no-check` | 13.7 ms | 16.1 ms | — |

### Indexing

| Case | Time | Peak RSS |
|---|---:|---:|
| Corpus full index (10 files) | 4.40 s | 2.26 GB |
| 200-file full index | 42.45 s | 1.54 GB |
| 200-file no-change index | 0.77 s | 594 MB |
| 200-file one-file modified | 1.19 s | 1.36 GB |
| 200-file one-file added | 0.88 s | — |
| 200-file one-file deleted | 0.81 s | — |
| 400-file full index | 81.76 s | 1.57 GB |
| Index size, 400 files / 4 400 symbols | 13.85 MB | — |

The no-change and deletion cases paid the embedding-model cost even though no
embedding was recomputed.

### Retrieval quality (real ONNX embeddings, 135 grounded tasks, hybrid)

| Category | P@1 | P@3 | R@5 | R@10 | Hit@5 | MRR |
|---|---:|---:|---:|---:|---:|---:|
| exact (35) | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| lexical (20) | 0.700 | 0.850 | 0.925 | 0.925 | 0.950 | 0.852 |
| semantic (20) | 0.550 | 0.750 | 0.825 | 0.900 | 0.950 | 0.804 |
| context (10) | 0.100 | 0.600 | 0.750 | 0.850 | 0.800 | 0.509 |
| ambiguity (10) | 0.700 | 0.900 | 0.900 | 0.900 | 0.900 | 0.783 |
| relation (15) | 0.933 | 1.000 | 1.000 | 1.000 | 1.000 | 0.967 |
| structural (15) | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| adversarial (10) | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| **all (135)** | **0.793** | **0.904** | **0.937** | **0.956** | **0.963** | **0.893** |

Hermetic fake-provider all: P@1 0.659, P@3 0.763, R@5 0.864, R@10 0.911,
Hit@5 0.896, MRR 0.763.

### MCP

| Measurement | Value |
|---|---:|
| Sequential hybrid calls (200) | 8.11 ms p50 / 8.41 ms p95 / 8.57 ms p99 |
| RSS after `initialize` | 591 MB |
| RSS after 200 calls | 597 MB |
| RSS slope | +0.029 MB/call |
| Success rate | 1.0000 |
| Malformed frames contained | 5/5 |
| Restart | OK |

The server initialized the embedding model before serving any request, even
when the client only used structural or lexical tools.

### Warm retrieval (in-process MCP, corpus, 50 samples)

Measured with the pre-optimization binary: model already loaded at startup.

| Operation | p50 | p95 | p99 |
|---|---:|---:|---:|
| `resolve` | 0.04 ms | 0.06 ms | 0.14 ms |
| lexical search | 0.13 ms | 0.16 ms | 0.22 ms |
| semantic search | 9.83 ms | 10.30 ms | 17.22 ms |
| hybrid search | 9.88 ms | 10.09 ms | 10.16 ms |
| context | 8.62 ms | 8.92 ms | 15.50 ms |
| similar | 0.14 ms | 0.16 ms | 0.20 ms |

RSS after `initialize`: 591 MB. First semantic query: 55 ms.
At 4 400 symbols, the semantic scan alone adds ≈8 ms to a warm query (18.2 ms
total); the remaining cost is one query embedding (~10 ms on this machine).

## Baseline defects confirmed by inspection + measurement

1. **Every `lx search`, including `--mode lexical`, constructed the ONNX model**
   (`Session::hydrate` → `FastEmbedProvider::with_cache_dir`). 766 ms and
   ~600 MB for an operation that never touches a vector.
2. **Every `lx mcp` process loaded the model at startup**, even if the client
   only used `resolve`/`relations`/`index_status`. Startup RSS 591 MB.
3. **No-change and deletion-only indexing loaded the model**, despite
   performing zero embeddings. 0.77 s / 594 MB.
4. **The workspace was read twice per index pass**: once in `tree_hash`, once in
   `scan_workspace`, both hashing every file.
5. **Every grammar-backed file was parsed and analyzed twice**:
   `parse_symbols` built one tree, then `Parser::parse` built another for
   `extract_relations`.
6. **Startup spawned `git` up to twice per command** (`rev-parse --show-toplevel`
   and `rev-parse HEAD`) to answer questions that are cheap filesystem reads.
7. **`Session::open_with` probed population with one engine, dropped it, then
   opened the substrate again** for the real session.
8. **Per-row SQLite inserts re-prepared each statement** inside transactions.
9. **CamelCase identifiers were opaque to lexical search**: `verifyPassword`
   was one token, so `password check` could not match it.
10. Semantic retrieval is an exact `O(symbols)` scan. A precomputed-norm
    micro-optimization was evaluated and rejected because it produced no
    measurable end-to-end change at 4 400 symbols.

## Methodology and limitations

- Every measurement used a release binary and a fresh process unless explicitly
  labelled warm.
- Percentiles use 20 samples for cold CLI operations, 50–200 for MCP, and 3
  samples for hybrid cold latency (model load dominates and is stable).
- Peak RSS comes from `/usr/bin/time -l` (`maximum resident set size`); MCP RSS
  comes from `ps -o rss=`.
- Quality metrics come from the checked-in 135-task corpus and truth set. The
  fake-provider run is a determinism/regression check only; the real-embedding
  run is the quality measurement.
- CPU time is reported where the harness records it; no direct energy
  measurement was performed, so no energy claims are made.
- The macOS 27 + ONNX Runtime combination in this environment has unusually
  high model-load memory. The model cost is therefore stated separately from
  Lynx's own footprint.
