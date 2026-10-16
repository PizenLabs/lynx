#!/usr/bin/env python3
"""Agent-utility probe: realistic repository-navigation workflows over MCP.

Runs a small set of agent-style tasks the way a client would — through the MCP
`tools/call` surface — and measures the work each requires:

    calls          number of retrieval calls to reach the answer
    latency_ms     wall-clock for the workflow
    top1/topk      whether the expected symbol was the first / among returned
    context_tokens compiled context size for the context workflow

The point is not a benchmark score but evidence that Lynx reduces
repository-navigation work: each workflow reaches a grounded coordinate in one
or two calls, with no repository scan by the client.

Usage:
    agent_workflow_probe.py <lx-binary> <workspace>
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

WORKFLOWS = [
    # (name, tool, arguments, expected_substring)
    ("find_authentication_implementation", "search",
     {"query": "authentication flow", "limit": 5}, "auth::"),
    ("find_token_validation", "resolve", {"name": "ValidateToken"}, "ValidateToken"),
    ("find_token_refresh", "search",
     {"query": "token refresh", "limit": 5}, "RefreshToken"),
    ("find_request_authorization", "search",
     {"query": "request authorization", "limit": 5}, "Authorize"),
    ("smallest_context_for_auth", "context",
     {"query": "jwt authentication", "token_budget": 2048}, "primary_evidence"),
]


class Server:
    def __init__(self, lx, workspace):
        self.proc = subprocess.Popen(
            [lx, "mcp"], cwd=workspace, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )
        assert self.proc.stdin and self.proc.stdout
        self._send({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}})
        self.proc.stdout.readline()

    def _send(self, obj):
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def call(self, name, arguments, rid):
        self._send({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}})
        return json.loads(self.proc.stdout.readline())

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def inner_text(response):
    try:
        return response["result"]["content"][0]["text"]
    except Exception:
        return ""


def main():
    if len(sys.argv) < 3:
        print("usage: agent_workflow_probe.py <lx-binary> <workspace>", file=sys.stderr)
        return 2
    lx = os.path.abspath(sys.argv[1])
    workspace = os.path.abspath(sys.argv[2])

    work = tempfile.mkdtemp(prefix="lynx-agent-")
    ws = os.path.join(work, "repo")
    shutil.copytree(workspace, ws)
    shutil.rmtree(os.path.join(ws, ".lynx"), ignore_errors=True)
    subprocess.run([lx, "index", "."], cwd=ws, capture_output=True, text=True)

    srv = Server(lx, ws)
    print("== agent workflow ==")
    print(f"{'workflow':<38} {'calls':>5} {'latency_ms':>11} {'hit':>4} {'detail'}")
    rid = 1
    for name, tool, arguments, expected in WORKFLOWS:
        t0 = time.perf_counter()
        response = srv.call(tool, arguments, rid)
        rid += 1
        latency = (time.perf_counter() - t0) * 1000
        text = inner_text(response)
        hit = expected in text
        detail = ""
        if tool in ("search", "resolve", "resolve_all"):
            try:
                payload = json.loads(text)
                items = payload if isinstance(payload, list) else [payload]
                fqdns = [i["identity"]["fqdn"] for i in items if i]
                detail = f"top={fqdns[0] if fqdns else None} n={len(fqdns)}"
            except Exception:
                detail = "parse_error"
        elif tool == "context":
            try:
                pkg = json.loads(text)
                detail = (f"tokens={pkg['estimated_tokens']} "
                          f"items={len(pkg['primary_evidence'])}")
            except Exception:
                detail = "parse_error"
        print(f"{name:<38} {1:>5} {latency:>11.2f} {str(hit):>4} {detail}")

    # A two-call navigation workflow: resolve a symbol, then read its callers.
    t0 = time.perf_counter()
    resolved = srv.call("resolve", {"name": "auth::AuthService::Login"}, rid)
    rid += 1
    try:
        anchor = json.loads(inner_text(resolved))
        symbol_hash = anchor["identity"]["content_hash"]
        relations = srv.call("relations", {"symbol_hash": symbol_hash, "kind": "Calls"}, rid)
        rid += 1
        edges = json.loads(inner_text(relations))
        detail = f"edges={len(edges)}"
        hit = len(edges) > 0
    except Exception:
        detail = "parse_error"
        hit = False
    latency = (time.perf_counter() - t0) * 1000
    print(f"{'find_callers_of_login':<38} {2:>5} {latency:>11.2f} {str(hit):>4} {detail}")

    srv.close()
    shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
