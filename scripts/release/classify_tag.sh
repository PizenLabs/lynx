#!/bin/sh
# Release tag classification — the ONE source of truth for "does this git tag
# publish production binaries?".
#
# It is invoked both by `.github/workflows/release.yml` (to gate the build and
# release jobs) and by the distribution test suite, so the workflow decision and
# the tests cannot drift: there is exactly one implementation of the rule.
#
# Rule (strict SemVer, never substring matching):
#
#   vMAJOR.MINOR.PATCH                       -> stable      (production release)
#   vMAJOR.MINOR.PATCH-PRERELEASE[+BUILD]    -> prerelease  (validation only)
#   anything else                            -> invalid     (fail closed)
#
#   stable      v1.0.0  v1.0.1  v2.10.3
#   prerelease  v1.0.1-rc.1  v1.0.1-alpha.1  v1.0.1-beta.2  v2.0.0-dev.1
#   invalid     v1  v1.0  v1.0.0foo  release-v1.0.0  v1.0.0/  1.0.0
#
# Usage:
#   scripts/release/classify_tag.sh <tag>
#
# Output (stdout): exactly one of `stable`, `prerelease`, `invalid`.
# Exit status:     0 for stable/prerelease, 1 for invalid.
#
# A likely-looking but malformed tag (`v1.0.0foo`) is reported `invalid`; it is
# never silently treated as stable.

set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: classify_tag.sh <tag>" >&2
    exit 2
fi

tag="$1"

invalid() {
    printf 'invalid\n'
    exit 1
}

# The `v` prefix is part of the contract: git tags are `vX.Y.Z`.
case "$tag" in
    v*) version="${tag#v}" ;;
    *) invalid ;;
esac

# Strict SemVer 2.0.0, with the leading `v` already stripped:
#   core        MAJOR.MINOR.PATCH          (no leading zeros)
#   prerelease  -<id>(.<id>)*              (identifiers: [0-9A-Za-z-]+)
#   build       +<id>(.<id>)*
# Numeric prerelease identifiers must not have leading zeros; that is checked
# separately below because POSIX ERE cannot express it in one pass.
semver_re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
if ! printf '%s' "$version" | grep -Eq "$semver_re"; then
    invalid
fi

# Reject leading zeros in numeric prerelease identifiers (SemVer 2.0.0 §9).
no_build="${version%%+*}"
case "$no_build" in
    *-*) pre="${no_build#*-}" ;;
    *) pre="" ;;
esac
if [ -n "$pre" ]; then
    if ! printf '%s' "$pre" | awk -F. '{
        for (i = 1; i <= NF; i++) {
            id = $i
            if (id ~ /^[0-9]+$/ && length(id) > 1 && substr(id, 1, 1) == "0") exit 1
        }
    }'; then
        invalid
    fi
fi

# Classification: a prerelease component exists iff there is a `-` before any
# `+` build metadata. Build metadata alone does not make a release a prerelease.
case "$no_build" in
    *-*) printf 'prerelease\n'; exit 0 ;;
    *)   printf 'stable\n';     exit 0 ;;
esac
