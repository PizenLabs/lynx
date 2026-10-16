#!/usr/bin/env python3
"""Fair baseline: ripgrep vs Lynx on the same grounded discovery tasks.

Lynx returns *symbols*; ripgrep returns *files*. To compare fairly we score both
at the file level: does the file that declares an expected symbol appear in the
returned set, and how quickly / how many files were inspected?

ripgrep has no relevance ranking, so "top-k" means the first k files in its
output order (path order). This asymmetry is intentional and reported.

Usage:
    rg_baseline.py <lx-binary> <corpus> <tasks.json> <truth.json> [--limit K]
"""
import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time

LINE_RE = re.compile(r"\[\s*[\d.]+\]\s+(\S+)\s+\([^)]*\)\s+(\S+?):(\d+)-")


def build_index(lx, corpus):
    work = tempfile.mkdtemp(prefix="lynx-rg-")
    ws = os.path.join(work, "repo")
    shutil.copytree(corpus, ws)
    shutil.rmtree(os.path.join(ws, ".lynx"), ignore_errors=True)
    subprocess.run([lx, "index", ".", "--force"], cwd=ws,
                   capture_output=True, text=True)
    conn = sqlite3.connect(os.path.join(ws, ".lynx", "graph.sqlite3"))
    fqdn_to_file = {f: p for f, p in conn.execute("select fqdn, file_path from symbols")}
    conn.close()
    return work, ws, fqdn_to_file


def rg_search(ws, terms, limit):
    args = ["rg", "--files-with-matches", "--no-messages", "-i"]
    for t in terms:
        args += ["-e", t]
    args.append(".")
    t0 = time.perf_counter()
    proc = subprocess.run(args, cwd=ws, capture_output=True, text=True)
    elapsed = time.perf_counter() - t0
    files = [line.lstrip("./") for line in proc.stdout.splitlines() if line.strip()]
    return files, elapsed, len(files)


def lynx_search(lx, ws, query, limit):
    t0 = time.perf_counter()
    proc = subprocess.run(
        [lx, "search", query, "--limit", str(limit)],
        cwd=ws, capture_output=True, text=True,
    )
    elapsed = time.perf_counter() - t0
    files = []
    for line in proc.stdout.splitlines():
        m = LINE_RE.match(line)
        if m:
            files.append(m.group(2))
    return files, elapsed, len(files)


def terms_for(query):
    return [t for t in re.split(r"[^A-Za-z0-9_]+", query) if t] or [query]


def hit_at(files, expected_files, k):
    return bool(set(files[:k]) & expected_files)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("corpus")
    ap.add_argument("tasks")
    ap.add_argument("truth")
    ap.add_argument("--limit", type=int, default=5)
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)
    args.corpus = os.path.abspath(args.corpus)
    args.tasks = os.path.abspath(args.tasks)
    args.truth = os.path.abspath(args.truth)

    work, ws, fqdn_to_file = build_index(args.lx, args.corpus)
    tasks = json.load(open(args.tasks))["tasks"]
    truth = json.load(open(args.truth))["truth"]

    cats = {}
    rg_lat, ln_lat = [], []
    for task in tasks:
        if task["primitive"] not in ("search", "resolve") or task.get("expect"):
            continue
        exp = truth[task["id"]]["expected"]
        expected_files = {fqdn_to_file[e] for e in exp if e in fqdn_to_file}
        if not expected_files:
            continue
        terms = runs = None
        if task["primitive"] == "resolve":
            rq = task["query"]
            terms = [rq.split("::")[-1].split(".")[-1]]
        else:
            terms = terms_for(task["query"])
        rg_files, rg_t, rg_n = rg_search(ws, terms, args.limit)
        ln_files, ln_t, ln_n = lynx_search(args.lx, ws, task["query"], args.limit)
        rg_lat.append(rg_t)
        ln_lat.append(ln_t)
        c = cats.setdefault(task["category"], {"n": 0, "rg1": 0, "rg5": 0, "ln1": 0, "ln5": 0})
        c["n"] += 1
        c["rg1"] += hit_at(rg_files, expected_files, 1)
        c["rg5"] += hit_at(rg_files, expected_files, args.limit)
        c["ln1"] += hit_at(ln_files, expected_files, 1)
        c["ln5"] += hit_at(ln_files, expected_files, args.limit)

    print("== rg vs lynx (file-level hit; symbol-level for Lynx mapped to file) ==")
    print(f"{'category':<12} {'n':>4} {'rg@1':>7} {'rg@5':>7} {'lynx@1':>7} {'lynx@5':>7}")
    for cat, c in sorted(cats.items()):
        n = max(1, c["n"])
        print(f"{cat:<12} {c['n']:>4} {c['rg1']/n:>7.3f} {c['rg5']/n:>7.3f} "
              f"{c['ln1']/n:>7.3f} {c['ln5']/n:>7.3f}")
    print(f"mean_rg_latency_ms: {sum(rg_lat)/max(1,len(rg_lat))*1000:.2f}")
    print(f"mean_lynx_latency_ms: {sum(ln_lat)/max(1,len(ln_lat))*1000:.2f}")
    print("note: ripgrep returns files in path order (no ranking); Lynx returns "
          "ranked symbols. Lynx latency includes process start + hydrate.")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
