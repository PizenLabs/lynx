#!/usr/bin/env bash
# Runs the full distribution test suite:
#   1. release_test.sh        — tag classification + version guards + wiring
#   2. install_test.sh        — installer behavior (hermetic, stubbed releases)
#   3. clean_machine_test.sh  — real binary install -> index -> query -> MCP
#   4. cargo_install_test.sh  — package every crate, registry-install pizen-lynx
#
# The clean-machine stage embeds with the real model, so it requires the
# embedding weights to be available (downloaded once on first index). The
# cargo-install stage compiles the dependency closure and needs network access.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
status=0
"$HERE/release_test.sh" || status=1
echo
"$HERE/install_test.sh" || status=1
echo
"$HERE/clean_machine_test.sh" ${1:+"$1"} || status=1
echo
"$HERE/cargo_install_test.sh" || status=1
exit "$status"
