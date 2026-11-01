# Architecture records

This directory holds the design, contracts, audits, and measured reports behind
Lynx. They are written for contributors and maintainers, not as a second user
manual.

Start with [Concepts](../concepts.md) for the product model. Use this directory
when you need to know **why** a boundary exists, **what** is contractually
guaranteed, or **how** a claim was measured.

## Start here

| Document | Kind | Read it for |
|---|---|---|
| [final-product-audit.md](final-product-audit.md) | Current architecture audit | One coherent description of the engine as built: ownership, lifecycle, state, failure boundaries. |
| [production-contract.md](production-contract.md) | **Normative** | Runtime invariants, layer boundaries, and bounds a consumer may rely on. |
| [production-readiness.md](production-readiness.md) | **Normative** | Capability matrix, status vocabulary, and the production gate. |
| [final-product-report.md](final-product-report.md) | Measured report | Retrieval quality, scaling, lifecycle, and resource numbers with the run command. |
| [mcp-compatibility.md](mcp-compatibility.md) | Validation matrix | Which MCP consumers were exercised and what each configuration needs. |
| [Performance and productization audit](../audit/lynx-performance-and-productization-report.md) | Dated measured report | Baseline and final numbers for the 2026-10 retrieval, startup, memory, and indexing pass. |

## Distribution and release

| Document | Kind | Read it for |
|---|---|---|
| [distribution-design.md](distribution-design.md) | Design | Public surface and stability, install channels, verification, and the crates.io publication graph. |
| [release-contract.md](release-contract.md) | **Normative** | Tag classification, version guards, artifact policy, installer rules, and fail-closed behaviour. |
| [release-readiness.md](release-readiness.md) | Operational status | Release state machine, target matrix, and publish order. |
| [distribution-audit.md](distribution-audit.md) | Dated audit | The distribution surface as it stood when the current design was written. |
| [distribution-release-report.md](distribution-release-report.md) | Dated report | What was validated for the distribution phase, per platform. |
| [release-closure-report.md](release-closure-report.md) | Dated report | Release-engineering closure: what changed and what was verified. |

## Historical audits and reports

These records explain how the current design was reached. They are **snapshots
of a commit and a machine**, and some describe problems that have since been
fixed. Do not treat them as current guarantees.

| Document | Covers |
|---|---|
| [hardening-report.md](hardening-report.md) | The hardening pass: measured changes after the production audit. |
| [production-audit.md](production-audit.md) | Phase 0–1 production audit at commit `e54c768`. |
| [product-validation-audit.md](product-validation-audit.md) | Independent-engine validation audit at commit `0064c37`. |
| [product-validation-report.md](product-validation-report.md) | Before/after validation measurements for that pass. |
| [lynx-discovery-engine.md](lynx-discovery-engine.md) | Early vision and positioning. Superseded by the README and Concepts. |
| [lynx-technical-specification.md](lynx-technical-specification.md) | Early architecture and retrieval specification. Superseded by the final-product audit and the protocol types. |

## Which document wins?

When documents disagree, use this order:

1. **The code and its tests** — especially `crates/lynx-protocol/tests/` and
   `tests/distribution/`.
2. **Normative contracts** — `production-contract.md` and `release-contract.md`.
3. **User documentation** — the README, `docs/cli.md`, `docs/mcp.md`, and
   `docs/concepts.md`.
4. **Design documents**.
5. **Audits and reports** — historical evidence, not current policy.

If a user-facing document and a contract disagree, that is a bug: fix both in
the same pull request.

## Adding or updating a record

- New design or contract documents go in this directory with a clear title and
  a short scope paragraph.
- Dated audits and reports keep their original measurements and date context.
  Do not rewrite history to match today's code; add a new report instead.
- Link new documents from this index, and update the map in
  [`docs/README.md`](../README.md) if you add a new category.
- Run `python3 scripts/docs/check_links.py` before committing.
