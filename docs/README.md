# Lynx documentation

Lynx is a deterministic repository-intelligence engine. It turns a repository
into stable symbols, exact coordinates, structural relations, and embeddings,
then exposes them through the `lx` CLI and a stdio MCP server.

**Lynx discovers. Consumers decide.**

## Start here

| Guide | Read it when you want to… |
|---|---|
| [Getting started](getting-started.md) | Install Lynx, index a repository, run the first query, and connect an agent. |
| [Concepts](concepts.md) | Understand symbols, evidence, retrieval modes, ranking, capability levels, and bounds. |
| [CLI reference](cli.md) | Look up a command, flag, environment variable, or exit code. |
| [MCP reference](mcp.md) | Configure Claude, Cursor, VS Code, OpenCode, Zed, or another MCP client. |
| [Troubleshooting](troubleshooting.md) | Fix an install, index, query, model-download, or MCP integration problem. |
| [FAQ](faq.md) | Get a short answer to a common question. |

## For integrators

The supported product surface is:

- the `lx` binary (package `pizen-lynx`);
- the stdio MCP server started by `lx mcp`;
- the JSON shapes documented in the CLI and MCP references.

The internal `pizen-lynx-*` crates are published only because Cargo requires a
registry-resolvable dependency graph for `cargo install pizen-lynx`. They are
**not** a public or stable API. See
[distribution design § Public surface and stability](architecture/distribution-design.md#13-public-surface-and-stability).

Key integration documents:

- [MCP tools and per-client configuration](mcp.md)
- [MCP compatibility matrix](architecture/mcp-compatibility.md)
- [Production contract](architecture/production-contract.md)
- [Capability and status vocabulary](architecture/production-readiness.md)

## For contributors

- [Contributing guide](../CONTRIBUTING.md) — setup, quality gates, commit style,
  adding a language, and release rules.
- [Code of Conduct](../CODE_OF_CONDUCT.md)
- [Security policy](../SECURITY.md)
- [Benchmarks](../benchmarks/README.md) — reproducible retrieval, scaling,
  lifecycle, and MCP measurements.
- [Changelog](../CHANGELOG.md) — release history.
- [Architecture records](architecture/README.md) — design documents, contracts,
  audits, and historical reports.

## Documentation map

```text
README.md                  product overview, install, quick start, limits
docs/
  getting-started.md       install -> index -> query -> MCP
  concepts.md              mental model and guarantees
  cli.md                   command reference
  mcp.md                   MCP tools and client setup
  troubleshooting.md       failure modes and fixes
  faq.md                   short answers
  architecture/
    README.md              index and status of every architecture record
    *-contract.md          normative contracts (production, release)
    *-design.md            design rationale
    *-audit.md             recorded audits
    *-report.md            measured reports and benchmark snapshots
  report-issues/           historical issue reports from early phases
CONTRIBUTING.md            development and contribution workflow
SECURITY.md                vulnerability reporting and scope
SUPPORT.md                 where to ask for help
CHANGELOG.md               release history
```

## Versioning and stability

Lynx is pre-1.0. The CLI commands and MCP tool names are intended to be stable
within a minor release line, but a minor version may contain breaking changes.
The serialized wire form of `pizen-lynx-protocol` is the stability boundary:
renaming or removing a serialized field is a breaking change. Index formats may
change between versions; `lx status` reports `incompatible` instead of
misreading an old index, and `lx index --force` rebuilds it.

Architecture audits and reports are **dated snapshots**. They explain how a
decision was reached and what was measured at that time; they are not a
substitute for the current README, CLI reference, or contracts.
