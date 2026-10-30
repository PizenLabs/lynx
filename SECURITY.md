# Security policy

## Supported versions

Lynx is pre-1.0. Security fixes are made on the latest published release line.

| Version | Supported |
|---|---|
| `0.2.x` | Yes |
| `< 0.2` | No — upgrade to `0.2.x` |

The `lx` binary and the `pizen-lynx` crate are the supported product surface.
The internal `pizen-lynx-*` crates are published only so Cargo can resolve
`cargo install pizen-lynx`; they are not a stable public API.

## Reporting a vulnerability

**Please do not open a public issue, pull request, or discussion for a security
problem.**

Use GitHub's private vulnerability reporting:

<https://github.com/PizenLabs/lynx/security/advisories/new>

Include, where possible:

- the affected version (`lx --version`) and platform;
- the install channel (installer, `cargo install`, source build);
- a minimal reproduction — a repository, file, query, or MCP frame;
- the impact you believe it has;
- whether the issue is already public anywhere.

If GitHub private reporting is unavailable, open a minimal issue asking for a
private contact channel. Do not include exploit details in that issue.

We will acknowledge a report as soon as practical, investigate it, and keep
you informed of the outcome. This is a volunteer-maintained project, so no
fixed response SLA is offered. We will credit reporters in the advisory unless
you ask to remain anonymous.

## Scope

Lynx runs locally and has a deliberately small attack surface:

- **Reads** the repository you point it at.
- **Writes** only `<workspace>/.lynx` and, on first index, the embedding-model
  cache at `$HOME/.cache/lynx/fastembed`.
- **Serves** MCP over stdio. It does not open a network listener.
- **Does not** send telemetry.
- The installer is HTTPS-only by default and verifies the release archive
  against the published `SHA256SUMS` before unpacking. (An explicit
  `LYNX_ALLOW_INSECURE` override exists for loopback testing only.)

In-scope examples:

- memory-safety or denial-of-service issues in parsing untrusted source files;
- path traversal or writing outside the workspace index directory;
- index corruption caused by a crafted repository;
- MCP protocol handling bugs (malformed frames, unbounded input, crashes);
- command injection or unsafe extraction in `install.sh`;
- release or installer integrity weaknesses.

Out-of-scope examples:

- vulnerabilities in the upstream embedding model, ONNX Runtime, tree-sitter
  grammars, SQLite, or Tantivy with no Lynx-specific exploit path — please
  report those upstream (and tell us if Lynx needs a mitigation);
- resource exhaustion from indexing an intentionally enormous repository,
  beyond the documented 8 MiB per-file bound and excluded-directory rules;
- issues that require an attacker to already control the user's shell,
  environment, or repository with equivalent privileges;
- missing hardening headers or policies that do not apply to a local CLI.

## Security-relevant design notes

- The index stores symbol metadata, source snippets, and vectors under
  `<workspace>/.lynx`. Treat it as sensitive as the source tree. Add `.lynx/`
  to `.gitignore` in repositories that do not already do so.
- Source snippets are read from the repository during indexing and stored in
  the index; retrieval does not re-read source files.
- Files larger than 8 MiB and the segments `.git`, `.lynx`,
  `.fastembed_cache`, `node_modules`, `vendor`, `target`, `build`, and `dist`
  are excluded from indexing. Test/mock/generated paths are excluded unless
  the index is built with `--include-tests`.
- Symlinks are not followed by the workspace walk.
- The MCP server inherits the permissions of the process that launches it. Run
  it as the same unprivileged user that owns the workspace.

## Disclosure

We will coordinate disclosure with the reporter. Once a fix is available, we
will publish a GitHub Security Advisory and, where relevant, a release note.
Please give us a reasonable opportunity to ship a fix before publishing details.
