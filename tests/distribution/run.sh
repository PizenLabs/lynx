#!/usr/bin/env bash
# Runs the full distribution test suite:
#   1. install_test.sh        — installer behavior (hermetic, stubbed releases)
#   2. clean_machine_test.sh  — real binary install -> index -> query -> MCP
#
# The clean-machine stage embeds with the real model, so it requires the
# embedding weights to be available (downloaded once on first index).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
status=0
"$HERE/install_test.sh" || status=1
echo
"$HERE/clean_machine_test.sh" ${1:+"$1"} || status=1
exit "$status"
