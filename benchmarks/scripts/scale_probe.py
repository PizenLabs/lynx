#!/usr/bin/env python3
"""Repository-scaling probe: index cost, persisted size, and cold query latency.

For each repository size (files x symbols-per-file) it measures the full product
path in fresh processes:

    generate -> lx index (wall / peak RSS) -> persisted size
             -> cold lx search p50/p95 (fresh process each call)
             -> cold lx resolve p50

The result is the evidence base for the declared supported-repository envelope:
it shows how index time and query latency scale with symbol count, so limits can
be stated from measurement rather than guessed.

Usage:
    scale_probe.py <lx-binary> [--sizes 50,100,200,400] [--symbols-per-file M]
                   [--queries N] [--workdir DIR]
"""
import argparse
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
        ["/usr/bin/time", "-l", *cmd], cwd=cwd, capture_output=True, text=True
    )
    stderr = proc.stderr
    rss = int(RSS_RE.search(stderr).group(1)) / 1048576.0 if RSS_RE.search(stderr) else -1.0
    real = float(REAL_RE.search(stderr).group(1)) if REAL_RE.search(stderr) else -1.0
    return proc.stdout, real, rss


def run_once(cmd, cwd):
    t0 = time.perf_counter()
    subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    return time.perf_counter() - t0


def percentile(values, pct):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round((pct / 100.0) * (len(ordered) - 1))))]


def dir_size(path):
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                pass
    return total


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("--sizes", default="50,100,200,400")
    ap.add_argument("--symbols-per-file", type=int, default=10)
    ap.add_argument("--queries", type=int, default=15)
    ap.add_argument("--workdir", default=None)
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)
    sizes = [int(s) for s in args.sizes.split(",") if s.strip()]

    work = args.workdir or tempfile.mkdtemp(prefix="lynx-scale-")
    own_work = args.workdir is None
    here = os.path.dirname(os.path.abspath(__file__))

    print("== repository scaling ==")
    print(f"symbols_per_file: {args.symbols_per_file}")
    print(f"{'files':>6} {'symbols':>8} {'index_s':>9} {'peak_rss_mb':>12} "
          f"{'index_mb':>9} {'search_p50_ms':>14} {'search_p95_ms':>14} {'resolve_p50_ms':>15}")

    for n in sizes:
        repo = os.path.join(work, f"repo{n}")
        if os.path.exists(repo):
            shutil.rmtree(repo)
        subprocess.run(
            [sys.executable, os.path.join(here, "gen_repo.py"), repo,
             "--files", str(n), "--symbols-per-file", str(args.symbols_per_file),
             "--lang", "rust"],
            check=True, capture_output=True,
        )
        _, real, rss = run_timed([args.lx, "index", "."], repo)
        size_mb = dir_size(os.path.join(repo, ".lynx")) / 1048576.0

        search_lat, resolve_lat = [], []
        for i in range(args.queries):
            search_lat.append(
                run_once([args.lx, "search", f"func_{i % 7}_{i % 5}", "--limit", "5"], repo)
            )
            resolve_lat.append(run_once([args.lx, "resolve", f"func_{i % 7}_{i % 5}"], repo))

        symbols = n * (args.symbols_per_file + 1)
        print(f"{n:>6} {symbols:>8} {real:>9.3f} {rss:>12.1f} {size_mb:>9.2f} "
              f"{percentile(search_lat, 50)*1000:>14.1f} "
              f"{percentile(search_lat, 95)*1000:>14.1f} "
              f"{percentile(resolve_lat, 50)*1000:>15.1f}", flush=True)

    if own_work:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
