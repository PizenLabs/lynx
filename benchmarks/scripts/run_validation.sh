#!/usr/bin/env bash
# Lynx product validation suite.
#
# Reproducibly measures:
#   - grounded discovery quality (135 tasks) across every primitive
#   - indexing scaling (wall time, peak RSS) across synthetic repo sizes
#   - persistence lifecycle (index / hydrate / cold-query latency, RSS)
#   - MCP stress (sequential + malformed + timeout + restart)
#   - a fair ripgrep baseline on the same tasks
#
# Usage: benchmarks/scripts/run_validation.sh [ROOT]
# Output: benchmarks/reports/validation_<timestamp>.txt
set -euo pipefail

ROOT="${1:-$(git rev-parse --show-toplevel)}"
cd "$ROOT"
SCRIPTS="$ROOT/benchmarks/scripts"
REPORTS="$ROOT/benchmarks/reports"
mkdir -p "$REPORTS"
TS="$(date +%Y%m%d_%H%M%S)"
REPORT="$REPORTS/validation_${TS}.txt"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "building release binaries..."
cargo build --release -p pizen-lynx >/dev/null 2>&1
LX="$ROOT/target/release/lx"

log() { echo "$@" | tee -a "$REPORT"; }

log "Lynx product validation report  ${TS}"
log "root:   ${ROOT}"
log "binary: ${LX}"
log ""

log "== discovery suite (hermetic fake provider) =="
cargo run --release -q -p pizen-lynx-core --example validation_suite \
  -- benchmarks/corpus/auth-service \
     benchmarks/tasks/discovery.json benchmarks/truth/discovery.json \
  2>>"$REPORT" | tee -a "$REPORT"
log ""

log "== discovery suite (real ONNX embeddings) =="
LYNX_REAL_EMBED=1 cargo run --release -q -p pizen-lynx-core --example validation_suite \
  -- benchmarks/corpus/auth-service \
     benchmarks/tasks/discovery.json benchmarks/truth/discovery.json \
  2>>"$REPORT" | tee -a "$REPORT"
log ""

log "== indexing scaling (files x 10 symbols, rust) =="
for n in 50 100 200 400; do
  repo="$WORK/r$n"
  python3 "$SCRIPTS/gen_repo.py" "$repo" --files "$n" --symbols-per-file 10 --lang rust >/dev/null
  run="$WORK/run$n"; mkdir -p "$run"
  out=$( cd "$run" && /usr/bin/time -l "$LX" index "$repo" --force 2>&1 )
  rss=$(echo "$out" | grep "maximum resident" | awk '{print $1}')
  real=$(echo "$out" | grep -E " real" | awk '{print $1}')
  log "files=$n  maxRSS=$(echo "scale=0; $rss/1048576" | bc)MB  real=${real}s"
done
log ""

log "== lifecycle (index / hydrate / cold query / RSS) =="
python3 "$SCRIPTS/lifecycle_probe.py" "$LX" benchmarks/corpus/auth-service \
  --queries 20 | tee -a "$REPORT"
log ""

log "== MCP stress =="
python3 "$SCRIPTS/mcp_stress.py" "$LX" benchmarks/corpus/auth-service \
  --calls 200 | tee -a "$REPORT"
log ""

log "== ripgrep baseline (same tasks) =="
python3 "$SCRIPTS/rg_baseline.py" "$LX" benchmarks/corpus/auth-service \
  benchmarks/tasks/discovery.json benchmarks/truth/discovery.json | tee -a "$REPORT"
log ""

echo "report: $REPORT"
