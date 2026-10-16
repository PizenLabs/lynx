#!/usr/bin/env python3
"""MCP stress probe: sequential, malformed, unknown-tool, timeout, and restart.

Spawns `lx mcp` over a workspace, issues a workload, and reports success rate,
latency percentiles, and RSS drift. Every stage ends by asserting the server is
still responsive, so a containment regression is visible.

Usage:
    mcp_stress.py <lx-binary> <workspace> [--calls N]
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time


def rss_mb(pid):
    out = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()
    return int(out) / 1024.0 if out else -1.0


class Server:
    def __init__(self, lx, workspace):
        self.proc = subprocess.Popen(
            [lx, "mcp"],
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
        line = self.proc.stdout.readline()
        return line

    def raw(self, text):
        self.proc.stdin.write(text + "\n")
        self.proc.stdin.flush()
        return self.proc.stdout.readline()

    def rss(self):
        return rss_mb(self.proc.pid)

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def percentile(values, pct):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round((pct / 100.0) * (len(ordered) - 1))))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lx")
    ap.add_argument("workspace")
    ap.add_argument("--calls", type=int, default=300)
    args = ap.parse_args()
    args.lx = os.path.abspath(args.lx)
    args.workspace = os.path.abspath(args.workspace)

    work = tempfile.mkdtemp(prefix="lynx-mcp-")
    ws = os.path.join(work, "repo")
    shutil.copytree(args.workspace, ws)
    shutil.rmtree(os.path.join(ws, ".lynx"), ignore_errors=True)
    subprocess.run([args.lx, "index", ".", "--force"], cwd=ws,
                   capture_output=True, text=True)

    srv = Server(args.lx, ws)
    start_rss = srv.rss()

    # -- sequential workload -------------------------------------------------
    latencies = []
    successes = 0
    for i in range(1, args.calls + 1):
        t0 = time.perf_counter()
        line = srv.call(
            "tools/call",
            {"name": "search", "arguments": {"query": f"token {i % 23}", "limit": 5}},
            i,
        )
        latencies.append(time.perf_counter() - t0)
        try:
            obj = json.loads(line)
            if obj.get("result", {}).get("isError") is False:
                successes += 1
        except Exception:
            pass
    end_rss = srv.rss()

    print("== mcp stress ==")
    print(f"sequential_calls: {args.calls}")
    print(f"success_rate: {successes / args.calls:.4f}")
    print(f"latency_p50_ms: {percentile(latencies, 50) * 1000:.2f}")
    print(f"latency_p95_ms: {percentile(latencies, 95) * 1000:.2f}")
    print(f"latency_p99_ms: {percentile(latencies, 99) * 1000:.2f}")
    print(f"rss_start_mb: {start_rss:.1f}")
    print(f"rss_end_mb: {end_rss:.1f}")
    print(f"rss_delta_mb: {end_rss - start_rss:+.2f}")
    print(f"rss_slope_mb_per_call: {(end_rss - start_rss) / args.calls:.5f}")

    # -- malformed frames ----------------------------------------------------
    malformed = 0
    for text in ["not json", "{", "[]", "null", '{"jsonrpc":"2.0"}']:
        try:
            srv.raw(text)
            malformed += 1
        except Exception:
            pass
    alive = srv.call("ping", {}, 9999)
    print(f"malformed_frames_contained: {malformed}")
    print(f"alive_after_malformed: {'result' in alive}")

    # -- unknown tool --------------------------------------------------------
    unknown = srv.call(
        "tools/call", {"name": "does_not_exist", "arguments": {}}, 10000
    )
    print(f"unknown_tool_rejected: {'error' in unknown}")
    print(f"alive_after_unknown: {'result' in srv.call('ping', {}, 10001)}")

    # -- zero timeout --------------------------------------------------------
    zero = srv.call(
        "tools/call",
        {"name": "search", "arguments": {"query": "auth", "timeout_ms": 0}},
        10002,
    )
    zero_is_error = "isError" in zero and "true" in zero
    print(f"zero_timeout_isError: {zero_is_error}")
    print(f"alive_after_timeout: {'result' in srv.call('ping', {}, 10003)}")

    # -- huge query ----------------------------------------------------------
    huge = srv.call(
        "tools/call",
        {"name": "search", "arguments": {"query": "token " * 2000, "limit": 5}},
        10004,
    )
    print(f"huge_query_answered: {'result' in huge}")
    print(f"alive_after_huge: {'result' in srv.call('ping', {}, 10005)}")

    srv.close()

    # -- restart -------------------------------------------------------------
    srv2 = Server(args.lx, ws)
    restarted = srv2.call("tools/call",
                          {"name": "search", "arguments": {"query": "token"}}, 1)
    print(f"restart_ok: {'result' in restarted}")
    srv2.close()

    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
