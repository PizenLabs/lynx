# Benchmarks

Reproducible before/after measurements for Lynx's runtime and retrieval
properties. Driven by `scripts/run_hardening.sh`.

## Running everything

```bash
cd "$(git rev-parse --show-toplevel)"
benchmarks/scripts/run_hardening.sh
```

Writes `benchmarks/reports/hardening_<timestamp>.txt` containing:

1. **Indexing scaling** — wall time and max RSS for synthetic repos of
   50/100/200/400 files × 10 symbols.
2. **Retrieval benchmark** — Precision@1 / Recall@5 / Hit@5 / MRR over a
   ground-truth corpus, with the hermetic fake provider and the real ONNX
   embedder.
3. **MCP RSS probe** — a long-lived server's RSS versus query count (detects
   per-query accumulation).

## Components

| Script / example | Purpose |
|---|---|
| `scripts/gen_repo.py` | Deterministic synthetic repo generator (rust/go/ts/py). |
| `scripts/mcp_rss_probe.py` | Samples `lx mcp` RSS across N queries; prints the slope. |
| `scripts/run_hardening.sh` | Orchestrates all of the above into one report. |
| `crates/lynx-core/examples/profile_index.rs` | Stage-by-stage indexing profiler (read/parse/graph/lex/embed). |
| `crates/lynx-core/examples/profile_embed.rs` | Batched vs per-file embedding cost. |
| `crates/lynx-core/examples/profile_embed_len.rs` | Embedding cost vs text length. |
| `crates/lynx-core/examples/retrieval_bench.rs` | Precision@K / Recall@K / MRR harness. |
| `retrieval-corpus/` | Multi-language ground-truth corpus (Go/TS/Python/Rust). |
| `retrieval-truth.json` | Ground-truth queries (kept **outside** the corpus so it is not indexed). |

## Individual runs

```bash
# Stage profile of indexing (fake, 384-d provider)
cargo run --release -p pizen-lynx-core --example profile_index -- /path/to/repo 200

# Stage profile with the real ONNX embedder
LYNX_REAL_EMBED=1 cargo run --release -p pizen-lynx-core --example profile_index -- /path/to/repo 200

# Retrieval quality (hermetic fake provider)
cargo run --release -p pizen-lynx-core --example retrieval_bench

# Retrieval quality (real embeddings)
LYNX_REAL_EMBED=1 cargo run --release -p pizen-lynx-core --example retrieval_bench

# Per-query explainability from the CLI
lx search "token refresh" --limit 5 --explain
```

## Notes

- The fake provider is deterministic but semantically meaningless; use it for
  regression/stability checks only. Use `LYNX_REAL_EMBED=1` for quality numbers.
- The real-embedding run loads `bge-small-en-v1.5` from
  `~/.cache/lynx/fastembed` (or `$HOME/.cache/lynx/fastembed`).
- The legacy `run_benchmarks.sh` (cargo bench) wrapper is superseded; there are
  no `benches/` targets.
