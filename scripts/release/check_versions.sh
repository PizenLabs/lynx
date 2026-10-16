#!/bin/sh
# Release version consistency — verify that a git tag, the resolved Cargo package
# version, and (optionally) a built binary all agree.
#
# This is the version-guard used by `.github/workflows/release.yml` (stable) and
# the prerelease validation path, and by the distribution test suite. It reads
# the *resolved* Cargo metadata (not a textual grep of Cargo.toml), so workspace
# inheritance and dependency resolution cannot hide a mismatch.
#
# Usage:
#   scripts/release/check_versions.sh <tag> [--manifest-path <Cargo.toml>]
#                                             [--package <name>]
#                                             [--binary <path/to/lx>]
#
# Checks, in order:
#   1. the tag is a valid stable/prerelease SemVer tag (via classify_tag.sh),
#   2. the tag's version equals the resolved `<package>` version,
#   3. if --binary is given, `<binary> --version` reports the same version.
#
# Exit status: 0 when every check passes, non-zero (fail closed) otherwise.
#
# Example:
#   scripts/release/check_versions.sh v0.2.0 --binary target/release/lx

set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
CLASSIFY="$HERE/classify_tag.sh"

MANIFEST=""
PACKAGE="pizen-lynx"
BINARY=""
TAG=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --manifest-path) shift; [ "$#" -gt 0 ] || { echo "check_versions: --manifest-path needs a value" >&2; exit 2; }; MANIFEST="$1" ;;
        --manifest-path=*) MANIFEST="${1#--manifest-path=}" ;;
        --package) shift; [ "$#" -gt 0 ] || { echo "check_versions: --package needs a value" >&2; exit 2; }; PACKAGE="$1" ;;
        --package=*) PACKAGE="${1#--package=}" ;;
        --binary) shift; [ "$#" -gt 0 ] || { echo "check_versions: --binary needs a value" >&2; exit 2; }; BINARY="$1" ;;
        --binary=*) BINARY="${1#--binary=}" ;;
        --) shift; break ;;
        -*) echo "check_versions: unknown option: $1" >&2; exit 2 ;;
        *) TAG="$1" ;;
    esac
    shift
done

[ -n "$TAG" ] || { echo "usage: check_versions.sh <tag> [--manifest-path Cargo.toml] [--package name] [--binary path]" >&2; exit 2; }

if [ -z "$MANIFEST" ]; then
    # Default to the workspace manifest at the repository root (scripts/release/../..).
    MANIFEST="$(cd "$HERE/../.." && pwd)/Cargo.toml"
fi

command -v cargo >/dev/null 2>&1 || { echo "check_versions: cargo is required" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "check_versions: python3 is required" >&2; exit 2; }

# 1. The tag must classify (fail closed on invalid).
kind="$(sh "$CLASSIFY" "$TAG")" || {
    echo "check_versions: FAIL: '${TAG}' is not a valid release tag" >&2
    exit 1
}
expected="${TAG#v}"

# 2. Tag version must equal the resolved package version.
actual="$(cargo metadata --no-deps --format-version 1 --manifest-path "$MANIFEST" 2>/dev/null \
    | python3 -c '
import json, sys
name = sys.argv[1]
data = json.load(sys.stdin)
for pkg in data["packages"]:
    if pkg["name"] == name:
        print(pkg["version"])
        break
' "$PACKAGE")"

if [ -z "$actual" ]; then
    echo "check_versions: FAIL: package '${PACKAGE}' not found in ${MANIFEST}" >&2
    exit 1
fi
if [ "$expected" != "$actual" ]; then
    echo "check_versions: FAIL: tag ${TAG} implies version ${expected}, but package ${PACKAGE} resolves to ${actual}" >&2
    exit 1
fi
echo "check_versions: tag ${TAG} (${kind}) == package ${PACKAGE} ${actual}"

# 3. Optional: the built binary must report the same version.
if [ -n "$BINARY" ]; then
    [ -x "$BINARY" ] || { echo "check_versions: FAIL: binary not executable: ${BINARY}" >&2; exit 1; }
    reported="$("$BINARY" --version | awk '{ print $NF }')"
    if [ "$reported" != "$expected" ]; then
        echo "check_versions: FAIL: ${BINARY} reports ${reported}, expected ${expected}" >&2
        exit 1
    fi
    echo "check_versions: binary ${BINARY} reports ${reported}"
fi

echo "check_versions: OK"
