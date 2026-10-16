#!/usr/bin/env bash
# Lynx final product validation.
#
# Reproducibly runs every product benchmark and writes one timestamped report:
#
#   retrieval quality        (135 grounded tasks, fake + real embeddings)
#   repository scaling       (index time / RSS / size + cold query latency)
#   incremental indexing     (update cost + freshness lifecycle)
#   persistence lifecycle    (index / hydrate / cold query / RSS)
#   MCP                      (sequential + malformed + timeout + restart)
#   agent utility            (realistic MCP workflows)
#   ripgrep baseline         (same grounded tasks, file level)
#
# Usage: benchmarks/scripts/run_final.sh [ROOT]
# Output: benchmarks/reports/final_<timestamp>.txt
set -euo pipefail

ROOT="${1:-$(git rev-parse --show-toplevel)}"
cd "$ROOT"
SCRIPTS="$ROOT/benchmarks/scripts"
REPORTS="$ROOT/benchmarks/reports"
mkdir -p "$REPORTS"
TS="$(date +%Y%m%d_%H%M%S)"
REPORT="$REPORTS/final_${TS}.txt"

echo "building release binaries..."
cargo build --release -p pizen-lynx >/dev/null 2>&1
LX="$ROOT/target/release/lx"
CORPUS="$ROOT/benchmarks/corpus/auth-service"

log() { echo "$@" | tee -a "$REPORT"; }

log "Lynx final product report  ${TS}"
log "root:    ${ROOT}"
log "binary:  ${LX}"
log "commit:  $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
log "rust:    $(rustc --version 2>/dev/null || echo unknown)"
log "uname:   $(uname -a)"
log ""

log "== retrieval quality (hermetic fake provider) =="
cargo run --release -q -p pizen-lynx-core --example validation_suite \
  -- "$CORPUS" benchmarks/tasks/discovery.json benchmarks/truth/discovery.json \
  2>>"$REPORT" | tee -a "$REPORT"
log ""

log "== retrieval quality (real ONNX embeddings) =="
LYNX_REAL_EMBED=1 cargo run --release -q -p pizen-lynx-core --example validation_suite \
  -- "$CORPUS" benchmarks/tasks/discovery.json benchmarks/truth/discovery.json \
  2>>"$REPORT" | tee -a "$REPORT"
log ""

log "== repository scaling =="
python3 "$SCRIPTS/scale_probe.py" "$LX" --sizes 50,100,200,400 | tee -a "$REPORT"
log ""

log "== incremental indexing =="
python3 "$SCRIPTS/incremental_probe.py" "$LX" --files 200 | tee -a "$REPORT"
log ""

log "== persistence lifecycle =="
python3 "$SCRIPTS/lifecycle_probe.py" "$LX" "$CORPUS" --queries 20 | tee -a "$REPORT"
log ""

log "== MCP stress =="
python3 "$SCRIPTS/mcp_stress.py" "$LX" "$CORPUS" --calls 200 | tee -a "$REPORT"
log ""

log "== agent workflow =="
python3 "$SCRIPTS/agent_workflow_probe.py" "$LX" "$CORPUS" | tee -a "$REPORT"
log ""

log "== ripgrep baseline =="
python3 "$SCRIPTS/rg_baseline.py" "$LX" "$CORPUS" \
  benchmarks/tasks/discovery.json benchmarks/truth/discovery.json | tee -a "$REPORT"
log ""

echo "report: $REPORT"
