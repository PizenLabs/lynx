#!/usr/bin/env python3
"""A minimal, independent MCP stdio client used for distribution validation.

It spawns `lx mcp`, performs the protocol handshake, discovers tools, and
invokes the repository-intelligence surface, then shuts the server down cleanly.
It is a *consumer*: it knows nothing about Lynx internals beyond the standard
MCP stdio contract and the advertised tool schemas.

Usage: mcp_client.py /path/to/lx /path/to/workspace
Exit status is non-zero on any contract violation.
"""
import json
import os
import subprocess
import sys
import time

EXPECTED_TOOLS = [
    "search",
    "resolve",
    "resolve_all",
    "inspect",
    "relations",
    "similar",
    "context",
    "index_status",
]


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: mcp_client.py /path/to/lx [workspace]", file=sys.stderr)
        return 2
    binary = sys.argv[1]
    workspace = sys.argv[2] if len(sys.argv) > 2 else os.getcwd()

    proc = subprocess.Popen(
        [binary, "mcp"],
        cwd=workspace,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    next_id = 0

    def call(method, params):
        nonlocal next_id
        next_id += 1
        request = {"jsonrpc": "2.0", "id": next_id, "method": method, "params": params}
        proc.stdin.write(json.dumps(request) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError(f"no response to {method}; server stderr:\n{proc.stderr.read()}")
        response = json.loads(line)
        if "error" in response:
            raise RuntimeError(f"{method} returned JSON-RPC error: {response['error']}")
        if response.get("id") != next_id:
            raise RuntimeError(f"{method} response id mismatch: {response.get('id')}")
        return response["result"]

    def tool(name, arguments):
        result = call("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(f"tool {name} returned isError: {result}")
        return json.loads(result["content"][0]["text"])

    t0 = time.time()
    init = call(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "lynx-dist-validation", "version": "1.0.0"},
        },
    )
    init_ms = (time.time() - t0) * 1000

    server = init.get("serverInfo", {})
    protocol = init.get("protocolVersion")
    if protocol != "2024-11-05":
        raise RuntimeError(f"unexpected protocolVersion: {protocol}")

    tools = call("tools/list", {})["tools"]
    names = [tool["name"] for tool in tools]
    if names != EXPECTED_TOOLS:
        raise RuntimeError(f"tool discovery mismatch: {names}")

    q0 = time.time()
    evidence = tool("search", {"query": "validate token", "limit": 5})
    search_ms = (time.time() - q0) * 1000
    if not isinstance(evidence, list) or not evidence:
        raise RuntimeError("search returned no evidence")

    resolved = tool("resolve", {"name": "ValidateToken"})
    if resolved is None:
        raise RuntimeError("resolve returned null for a known symbol")

    package = tool("context", {"query": "token validation", "token_budget": 2048})
    if "estimated_tokens" not in package:
        raise RuntimeError("context package missing estimated_tokens")

    status = tool("index_status", {})
    if not status.get("file_count"):
        raise RuntimeError("index_status reported no files")

    # Clean shutdown: close stdin and require a zero exit.
    proc.stdin.close()
    try:
        code = proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise RuntimeError("server did not shut down within 30s")
    if code != 0:
        raise RuntimeError(f"server exited non-zero: {code}; stderr:\n{proc.stderr.read()}")

    summary = {
        "protocol": protocol,
        "server": server,
        "tools": names,
        "init_ms": round(init_ms, 1),
        "search_ms": round(search_ms, 1),
        "search_top": evidence[0]["identity"]["fqdn"],
        "resolve": resolved["identity"]["fqdn"],
        "context_tokens": package["estimated_tokens"],
        "index_files": status["file_count"],
        "shutdown": "clean",
    }
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:  # noqa: BLE001 - report and fail the test
        print(f"mcp_client: FAIL: {error}", file=sys.stderr)
        sys.exit(1)
