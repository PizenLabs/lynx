#!/usr/bin/env python3
"""Probe a long-lived `lx mcp` server for per-query resource accumulation.

Spawns the server over a workspace, issues a stream of search/context calls,
and samples RSS after every request. Prints a time series and the slope of RSS
versus query count. A flat slope means queries do not accumulate resources.

Usage:
    mcp_rss_probe.py <lx-binary> <workspace> <count> [--sample-every N]
"""
import argparse
import json
import subprocess
import sys


def rss_mb(pid: int) -> float:
    out = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()
    return int(out) / 1024.0


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("workspace")
    ap.add_argument("count", type=int)
    ap.add_argument("--sample-every", type=int, default=1)
    args = ap.parse_args()

    proc = subprocess.Popen(
        [args.lx, "mcp"],
        cwd=args.workspace,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
    )
    assert proc.stdin and proc.stdout

    # MCP handshake.
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}}) + "\n")
    proc.stdin.flush()
    proc.stdout.readline()

    samples = []
    for i in range(1, args.count + 1):
        req = {
            "jsonrpc": "2.0",
            "id": i,
            "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": f"handler_{i % 997}", "limit": 10}},
        }
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        proc.stdout.readline()
        if i % args.sample_every == 0 or i == args.count:
            samples.append((i, rss_mb(proc.pid)))

    proc.stdin.close()
    proc.wait()

    print(f"queries={args.count}")
    for i, mb in samples:
        print(f"  after {i:5d} queries: RSS {mb:8.1f} MB")
    if len(samples) >= 2:
        first = samples[0][1]
        last = samples[-1][1]
        print(f"delta RSS = {last - first:+.1f} MB over {samples[-1][0] - samples[0][0]} queries")
        print(f"slope     = {(last - first) / max(1, samples[-1][0] - samples[0][0]):.4f} MB/query")


if __name__ == "__main__":
    sys.exit(main())
