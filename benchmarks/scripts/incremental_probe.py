#!/usr/bin/env python3
"""Incremental-indexing probe: cost of update vs full rebuild, and freshness.

Measures, on a synthetic repository:

    full index           -> wall time / peak RSS
    re-index (no change) -> wall time / peak RSS (must be cheap: no re-embed)
    modify one file      -> wall time (must re-embed only that file)
    add one file
    delete one file
    rename one file

and asserts the freshness lifecycle:

    fresh  -> edit -> stale -> index -> fresh

Every stage runs `lx index` in a fresh process (the product path) and samples
peak RSS with `/usr/bin/time -l`.

Usage:
    incremental_probe.py <lx-binary> [--files N] [--symbols-per-file M]
                         [--workdir DIR]
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

RSS_RE = re.compile(r"(\d+)\s+maximum resident set size")
REAL_RE = re.compile(r"([\d.]+)\s+real")


def run_timed(cmd, cwd):
    proc = subprocess.run(
        ["/usr/bin/time", "-l", *cmd], cwd=cwd, capture_output=True, text=True
    )
    stderr = proc.stderr
    rss = int(RSS_RE.search(stderr).group(1)) / 1048576.0 if RSS_RE.search(stderr) else -1.0
    real = float(REAL_RE.search(stderr).group(1)) if REAL_RE.search(stderr) else -1.0
    return proc.stdout.strip(), real, rss, proc.returncode


def freshness(lx, cwd):
    out, _, _, _ = run_timed([lx, "status"], cwd)
    for line in out.splitlines():
        if line.startswith("freshness"):
            return line.split()[-1]
    return "?"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("--files", type=int, default=200)
    ap.add_argument("--symbols-per-file", type=int, default=10)
    ap.add_argument("--workdir", default=None)
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)

    work = args.workdir or tempfile.mkdtemp(prefix="lynx-incremental-")
    own_work = args.workdir is None
    repo = os.path.join(work, "repo")
    if os.path.exists(repo):
        shutil.rmtree(repo)
    here = os.path.dirname(os.path.abspath(__file__))
    subprocess.run(
        [sys.executable, os.path.join(here, "gen_repo.py"), repo,
         "--files", str(args.files), "--symbols-per-file", str(args.symbols_per_file),
         "--lang", "rust"],
        check=True, capture_output=True,
    )

    print("== incremental indexing ==")
    print(f"repo: {repo}")
    print(f"files: {args.files}  symbols-per-file: {args.symbols_per_file}")

    out, full_s, full_rss, _ = run_timed([args.lx, "index", "."], repo)
    print(f"full_index_time_s: {full_s:.3f}")
    print(f"full_index_peak_rss_mb: {full_rss:.1f}")

    out, noop_s, noop_rss, _ = run_timed([args.lx, "index", "."], repo)
    print(f"nochange_index_time_s: {noop_s:.3f}")
    print(f"nochange_index_peak_rss_mb: {noop_rss:.1f}")
    print(f"nochange_speedup_x: {full_s / max(noop_s, 1e-6):.1f}")

    # Modify one file.
    target = os.path.join(repo, "pkg0", "mod_0.rs")
    with open(target, "a") as fh:
        fh.write("\npub fn touched_0() -> i64 { 42 }\n")
    out, mod_s, mod_rss, _ = run_timed([args.lx, "index", "."], repo)
    print(f"one_file_modified_index_time_s: {mod_s:.3f}")
    print(f"one_file_modified_peak_rss_mb: {mod_rss:.1f}")
    print(f"one_file_modified_line: {[l for l in out.splitlines() if 'Indexed' in l]}")
    print(f"one_file_modified_speedup_vs_full_x: {full_s / max(mod_s, 1e-6):.1f}")

    # Add one file.
    with open(os.path.join(repo, "pkg0", "added.rs"), "w") as fh:
        fh.write("pub fn added() -> i64 { 7 }\n")
    out, add_s, _, _ = run_timed([args.lx, "index", "."], repo)
    print(f"one_file_added_index_time_s: {add_s:.3f}")
    print(f"one_file_added_line: {[l for l in out.splitlines() if 'Indexed' in l]}")

    # Delete the added file.
    os.remove(os.path.join(repo, "pkg0", "added.rs"))
    out, del_s, _, _ = run_timed([args.lx, "index", "."], repo)
    print(f"one_file_deleted_index_time_s: {del_s:.3f}")
    print(f"one_file_deleted_line: {[l for l in out.splitlines() if 'Indexed' in l]}")

    # Freshness lifecycle.
    print("\n== freshness lifecycle ==")
    print(f"freshness_after_index: {freshness(args.lx, repo)}")
    with open(target, "a") as fh:
        fh.write("\npub fn touched_again() -> i64 { 43 }\n")
    print(f"freshness_after_edit: {freshness(args.lx, repo)}")
    run_timed([args.lx, "index", "."], repo)
    print(f"freshness_after_reindex: {freshness(args.lx, repo)}")

    if own_work:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
