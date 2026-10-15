#!/usr/bin/env bash
# Hardening benchmark harness: reproducible before/after measurements for
# indexing cost, per-query memory behaviour, and retrieval quality.
#
# Usage:
#   benchmarks/scripts/run_hardening.sh [REPO_ROOT]
#
# Produces benchmarks/reports/hardening_<timestamp>.txt with:
#   - indexing scaling (wall time, max RSS) across synthetic repo sizes
#   - retrieval benchmark metrics (Precision@1 / Recall@5 / Hit@5 / MRR)
#     using the fake (hermetic) and real (ONNX) embedding providers
#   - a long-lived MCP server RSS-vs-query-count probe
set -euo pipefail

ROOT="${1:-$(git rev-parse --show-toplevel)}"
cd "$ROOT"
SCRIPTS="$ROOT/benchmarks/scripts"
REPORTS="$ROOT/benchmarks/reports"
mkdir -p "$REPORTS"
TS="$(date +%Y%m%d_%H%M%S)"
REPORT="$REPORTS/hardening_${TS}.txt"
WORK="$(mktemp -d)"

echo "building release binaries..."
cargo build --release -p pizen-lynx >/dev/null 2>&1
LX="$ROOT/target/release/lx"

log() { echo "$@" | tee -a "$REPORT"; }

log "Lynx hardening report  ${TS}"
log "root: ${ROOT}"
log ""

log "== indexing scaling (files x 10 symbols) =="
for n in 50 100 200 400; do
  repo="$WORK/r$n"
  python3 "$SCRIPTS/gen_repo.py" "$repo" --files "$n" --symbols-per-file 10 --lang rust >/dev/null
  mkdir -p "$WORK/run$n"
  ( cd "$WORK/run$n" && rm -rf .lynx )
  out=$( cd "$WORK/run$n" && /usr/bin/time -l "$LX" index "$repo" --force 2>&1 )
  rss=$(echo "$out" | grep "maximum resident" | awk '{print $1}')
  real=$(echo "$out" | grep -E " real" | awk '{print $1}')
  log "files=$n  maxRSS=$(echo "scale=0; $rss/1048576" | bc)MB  real=${real}s"
done
log ""

log "== retrieval benchmark (fake provider, hermetic) =="
cargo run --release -q -p pizen-lynx-core --example retrieval_bench 2>/dev/null | tee -a "$REPORT" | tail -6
log ""

log "== retrieval benchmark (real ONNX embeddings) =="
LYNX_REAL_EMBED=1 cargo run --release -q -p pizen-lynx-core --example retrieval_bench 2>/dev/null | tee -a "$REPORT" | tail -6
log ""

log "== MCP long-lived RSS probe (400 files x 10 symbols, 300 queries) =="
repo="$WORK/mcp"
python3 "$SCRIPTS/gen_repo.py" "$repo" --files 400 --symbols-per-file 10 --lang rust >/dev/null
python3 "$SCRIPTS/mcp_rss_probe.py" "$LX" "$repo" 300 --sample-every 50 | tee -a "$REPORT"
log ""

rm -rf "$WORK"
echo "report: $REPORT"
