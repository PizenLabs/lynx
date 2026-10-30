# Support

## Read this first

- [Getting started](docs/getting-started.md) — install, first index, first query,
  MCP setup.
- [CLI reference](docs/cli.md) — every command and flag.
- [MCP reference](docs/mcp.md) — tools and per-client configuration.
- [Concepts](docs/concepts.md) — symbols, evidence, ranking, limits.
- [Troubleshooting](docs/troubleshooting.md) — common failures and fixes.
- [FAQ](docs/faq.md) — short answers to recurring questions.

`lx doctor` is the fastest way to diagnose an installation or workspace:

```bash
lx --version
lx doctor
```

## Where to ask

| Need | Where |
|---|---|
| Reproducible bug | [Open a bug report](https://github.com/PizenLabs/lynx/issues/new?template=bug_report.yml) |
| Feature proposal | [Open a feature request](https://github.com/PizenLabs/lynx/issues/new?template=feature_request.yml) |
| Usage question | [Open a question](https://github.com/PizenLabs/lynx/issues/new?template=question.yml) |
| Security issue | **Privately** via [SECURITY.md](SECURITY.md) — never a public issue |
| Documentation fix | Open a pull request; see [CONTRIBUTING.md](CONTRIBUTING.md) |

GitHub Discussions are not enabled for this repository. Questions that turn out
to be bugs will be moved to the bug tracker.

## Before opening an issue

1. Search existing and closed issues.
2. Upgrade to the latest release and re-run `lx index`.
3. Run `lx doctor` and copy its output.
4. Reduce the problem to the smallest repository or command that reproduces it.
5. Remove secrets, private paths, and proprietary source from logs.

Include:

- `lx --version` and platform (`uname -a` or OS/arch);
- install channel (installer, `cargo install`, source build);
- the exact command and its output;
- whether the index is fresh, stale, missing, incompatible, or corrupt;
- relevant MCP client and version, for integration issues.

## Scope of support

Lynx is maintained on a best-effort basis. There is no commercial support
contract and no guaranteed response time. Supported platforms are macOS arm64
and Linux x86_64/arm64; see the platform table in the
[README](README.md#supported-platforms). macOS Intel and Windows are
unsupported.

## Commercial and integration questions

Lynx is standalone and consumer-agnostic. It does not provide an SDK, a hosted
service, or vendor-specific integrations. For integration questions, open an
issue describing the consumer and the protocol shape you need.
