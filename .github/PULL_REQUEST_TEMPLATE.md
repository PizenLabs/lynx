## Summary

<!-- What problem does this change solve? One or two sentences. -->

Closes #

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Performance or retrieval-quality improvement
- [ ] Refactor (no behaviour change)
- [ ] Documentation
- [ ] CI / distribution / release

## What changed

<!--
List the substantive changes. Call out anything a reviewer should look at
first, and anything intentionally left out.
-->

## Testing

<!-- Commands run, and the result. Add benchmark numbers for perf/ranking work. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace`
- [ ] `cargo test --workspace`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `python3 scripts/docs/check_links.py` (docs changes)
- [ ] Distribution tests / benchmarks (when relevant)

## Compatibility

- [ ] No user-visible change.
- [ ] CLI or MCP output changed; docs updated.
- [ ] Wire contract (`pizen-lynx-protocol`) changed; wire tests and docs updated.
- [ ] Index format changed; existing indexes are handled or report
      `incompatible`.
- [ ] Release or installer behaviour changed; release tests updated.

## Documentation

- [ ] README / docs updated where behaviour, limits, or installation changed.
- [ ] No documentation change is needed.

## Checklist

- [ ] I read [CONTRIBUTING.md](../CONTRIBUTING.md).
- [ ] I added or updated tests for the behaviour I changed.
- [ ] I kept the change focused; unrelated refactors are separate pull requests.
- [ ] I removed secrets, private paths, and proprietary source from the diff.
