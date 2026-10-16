#!/usr/bin/env python3
"""Lifecycle probe: index / hydrate / cold-query latency and peak RSS.

Measures the exact product lifecycle:

    lx index  ->  persist  ->  process exits  ->  new process  ->  query

Every query runs in a *fresh process* against the persisted `.lynx` index, so
the reported latency is the true cold-start path (hydrate + query), never a
per-query re-index. Peak RSS is sampled with `/usr/bin/time -l`.

Usage:
    lifecycle_probe.py <lx-binary> <workspace> [--queries N]
"""
import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

RSS_RE = re.compile(r"(\d+)\s+maximum resident set size")
REAL_RE = re.compile(r"([\d.]+)\s+real")


def run_timed(cmd, cwd):
    proc = subprocess.run(
        ["/usr/bin/time", "-l", *cmd],
        cwd=cwd,
        capture_output=True,
        text=True,
    )
    stderr = proc.stderr
    rss = int(RSS_RE.search(stderr).group(1)) / 1048576.0 if RSS_RE.search(stderr) else -1.0
    real = float(REAL_RE.search(stderr).group(1)) if REAL_RE.search(stderr) else -1.0
    return proc.stdout, real, rss


def percentile(values, pct):
    if not values:
        return 0.0
    ordered = sorted(values)
    idx = min(len(ordered) - 1, int(round((pct / 100.0) * (len(ordered) - 1))))
    return ordered[idx]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("workspace")
    ap.add_argument("--queries", type=int, default=40)
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)
    args.workspace = os.path.abspath(args.workspace)

    work = tempfile.mkdtemp(prefix="lynx-lifecycle-")
    ws = os.path.join(work, "repo")
    shutil.copytree(args.workspace, ws)
    shutil.rmtree(os.path.join(ws, ".lynx"), ignore_errors=True)

    print("== lifecycle ==")
    print(f"workspace: {ws}")

    _, index_real, index_rss = run_timed(
        [args.lx, "index", ".", "--force"], ws
    )
    print(f"index_time_s: {index_real:.3f}")
    print(f"index_peak_rss_mb: {index_rss:.1f}")

    # Cold query latency: each invocation is a fresh process hydrating `.lynx`.
    search_latency = []
    resolve_latency = []
    search_rss = []
    for i in range(args.queries):
        q = f"token validation {i % 7}"
        _, real, rss = run_timed([args.lx, "search", q, "--limit", "5"], ws)
        search_latency.append(real)
        search_rss.append(rss)
    for i in range(args.queries):
        _, real, _ = run_timed([args.lx, "resolve", "ValidateToken"], ws)
        resolve_latency.append(real)

    print(f"cold_search_n: {len(search_latency)}")
    print(f"cold_search_p50_s: {percentile(search_latency, 50):.3f}")
    print(f"cold_search_p95_s: {percentile(search_latency, 95):.3f}")
    print(f"cold_search_p99_s: {percentile(search_latency, 99):.3f}")
    print(f"cold_search_peak_rss_mb: {max(search_rss):.1f}")
    print(f"cold_resolve_p50_s: {percentile(resolve_latency, 50):.3f}")
    print(f"cold_resolve_p99_s: {percentile(resolve_latency, 99):.3f}")
    print(
        "note: cold_search includes process start + ONNX model load; "
        "cold_resolve loads no model"
    )

    # Persistence proof: with the source tree removed, a fresh process must
    # still answer from the persisted index — which is only possible if the
    # query path neither re-reads nor re-indexes the workspace.
    db_path = os.path.join(ws, ".lynx", "graph.sqlite3")
    db_mtime = os.path.getmtime(db_path)
    for root, dirs, files in os.walk(ws):
        if ".lynx" in root.split(os.sep):
            continue
        for f in files:
            os.remove(os.path.join(root, f))
    out, _, _ = run_timed([args.lx, "search", "token validation", "--limit", "5"], ws)
    print(f"query_without_sources_returns_results: {len(out.strip()) > 0}")
    print(f"db_unmodified_by_query: {os.path.getmtime(db_path) == db_mtime}")

    # Report persisted index size.
    total = 0
    for root, _, files in os.walk(os.path.join(ws, ".lynx")):
        for f in files:
            total += os.path.getsize(os.path.join(root, f))
    print(f"index_size_mb: {total / 1048576.0:.2f}")

    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
