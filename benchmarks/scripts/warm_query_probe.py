#!/usr/bin/env python3
"""Warm retrieval probe: per-operation latency inside one live MCP server.

Cold CLI latency mixes process start, index hydration, and (for semantic
operations) ONNX model construction. This probe separates those costs from the
query itself:

    start `lx mcp` -> initialize -> one warm-up hybrid query -> measure

It reports RSS immediately after `initialize` (before any embedding call) and
after warm-up, so the lazy model boundary is visible, plus p50/p95/p99 for each
primitive.

Usage:
    warm_query_probe.py <lx-binary> <workspace> [--calls N]
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time


class Server:
    def __init__(self, lx, workspace):
        self.proc = subprocess.Popen(
            [lx, "mcp", workspace],
            cwd=workspace,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        assert self.proc.stdin and self.proc.stdout
        self._send({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}})
        self.proc.stdout.readline()

    def _send(self, obj):
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params, rid):
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        return self.proc.stdout.readline()

    def rss_mb(self):
        out = subprocess.check_output(
            ["ps", "-o", "rss=", "-p", str(self.proc.pid)], text=True
        ).strip()
        return int(out) / 1024.0 if out else -1.0

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def percentile(values, pct):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round((pct / 100.0) * (len(ordered) - 1))))]


def bench(server, name, params, calls, start_id):
    latencies = []
    for index in range(calls):
        t0 = time.perf_counter()
        line = server.call("tools/call", params, start_id + index)
        latencies.append((time.perf_counter() - t0) * 1000.0)
        try:
            body = json.loads(line)
        except json.JSONDecodeError:
            continue
        if body.get("result", {}).get("isError") is True:
            print(f"{name}_error: {body['result']['content'][0]['text'][:200]}")
    print(
        f"{name} p50_ms={statistics.median(latencies):.2f} "
        f"p95_ms={percentile(latencies, 95):.2f} "
        f"p99_ms={percentile(latencies, 99):.2f} n={len(latencies)}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("workspace")
    ap.add_argument("--calls", type=int, default=50)
    ap.add_argument("--symbol", default="ValidateToken")
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)
    args.workspace = os.path.abspath(args.workspace)

    server = Server(args.lx, args.workspace)
    rss_start = server.rss_mb()
    print("== warm query probe ==")
    print(f"workspace: {args.workspace}")
    print(f"rss_after_initialize_mb: {rss_start:.1f}")

    t0 = time.perf_counter()
    server.call("tools/call", {"name": "search", "arguments": {"query": "token"}}, 1)
    first_semantic = (time.perf_counter() - t0) * 1000.0
    print(f"first_semantic_query_ms: {first_semantic:.1f}")
    print(f"rss_after_semantic_mb: {server.rss_mb():.1f}")

    bench(
        server,
        "resolve",
        {"name": "resolve", "arguments": {"name": args.symbol}},
        args.calls,
        100,
    )
    bench(
        server,
        "lexical",
        {
            "name": "search",
            "arguments": {"query": "token validation", "mode": "lexical", "limit": 10},
        },
        args.calls,
        200,
    )
    bench(
        server,
        "semantic",
        {
            "name": "search",
            "arguments": {"query": "decide whether a token is valid", "mode": "semantic", "limit": 10},
        },
        args.calls,
        300,
    )
    bench(
        server,
        "hybrid",
        {
            "name": "search",
            "arguments": {"query": "decide whether a token is valid", "mode": "hybrid", "limit": 10},
        },
        args.calls,
        400,
    )
    bench(
        server,
        "context",
        {"name": "context", "arguments": {"query": "token validation", "token_budget": 2048}},
        args.calls,
        500,
    )

    resolved = json.loads(
        server.call(
            "tools/call", {"name": "resolve", "arguments": {"name": args.symbol}}, 600
        )
    )
    hash_value = json.loads(resolved["result"]["content"][0]["text"])["identity"][
        "content_hash"
    ]
    bench(
        server,
        "similar",
        {"name": "similar", "arguments": {"symbol_hash": hash_value}},
        args.calls,
        700,
    )
    print(f"rss_end_mb: {server.rss_mb():.1f}")
    server.close()


if __name__ == "__main__":
    sys.exit(main())
