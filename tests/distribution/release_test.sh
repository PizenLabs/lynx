#!/usr/bin/env bash
# Release-mode distribution tests.
#
# Proves the release *decision* and the release *wiring*:
#   1. tag classification (stable / prerelease / invalid) — the required table,
#   2. fail-closed behavior for malformed tags,
#   3. tag <-> Cargo package version <-> binary version consistency (and the
#      mismatch failure mode),
#   4. that `.github/workflows/release.yml` can only publish production binaries
#      from a stable tag — a prerelease tag has no path to `gh release create`,
#   5. unsupported platforms are rejected and never released.
#
# The classification rule lives in `scripts/release/classify_tag.sh`, which the
# workflow itself invokes; this test therefore exercises the real release logic,
# not a duplicate.
#
# Requirements: bash, python3, cargo (for the version-consistency checks).
# Usage: tests/distribution/release_test.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLASSIFY="$ROOT/scripts/release/classify_tag.sh"
CHECK="$ROOT/scripts/release/check_versions.sh"
WORKFLOW="$ROOT/.github/workflows/release.yml"
INSTALLER="$ROOT/install.sh"

failures=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

[ -f "$CLASSIFY" ] || { echo "missing classifier: $CLASSIFY" >&2; exit 2; }
[ -f "$CHECK" ] || { echo "missing version guard: $CHECK" >&2; exit 2; }
[ -f "$WORKFLOW" ] || { echo "missing workflow: $WORKFLOW" >&2; exit 2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lynx-release.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# --- 1. classification table ------------------------------------------------

echo "[classification]"

expect_kind() { # tag expected_kind
    local tag="$1" want="$2" got status
    got="$(sh "$CLASSIFY" "$tag" 2>/dev/null)"
    status=$?
    if [ "$got" = "$want" ]; then
        if [ "$want" = "invalid" ]; then
            if [ "$status" -ne 0 ]; then pass "$tag -> $want (non-zero exit)"; else fail "$tag -> $want but exited 0"; fi
        else
            if [ "$status" -eq 0 ]; then pass "$tag -> $want"; else fail "$tag -> $want but exited $status"; fi
        fi
    else
        fail "$tag -> expected $want, got '$got' (status=$status)"
    fi
}

for tag in v1.0.0 v1.0.1 v2.10.3 v0.2.0; do
    expect_kind "$tag" stable
done

for tag in v1.0.1-rc v1.0.1-rc.1 v1.0.1-alpha v1.0.1-alpha.1 \
           v1.0.1-beta v1.0.1-beta.2 v2.0.0-dev.1 v2.0.0-preview.1; do
    expect_kind "$tag" prerelease
done

for tag in v1 v1.0 v1.0.0foo release-v1.0.0 "v1.0.0/" 1.0.0 v01.0.0 \
           v1.0.0- v1.0.0-01 v1.0.0+ "v1.0.0-rc..1"; do
    expect_kind "$tag" invalid
done

# --- 2. version consistency -------------------------------------------------

echo "[version consistency]"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
[ -n "$VERSION" ] || { echo "could not read workspace version" >&2; exit 2; }

if sh "$CHECK" "v${VERSION}" >/dev/null 2>&1; then
    pass "tag v${VERSION} matches the resolved package version"
else
    fail "tag v${VERSION} did not match the resolved package version"
    sh "$CHECK" "v${VERSION}" 2>&1 | sed 's/^/        /'
fi

if sh "$CHECK" "v999.0.0" >/dev/null 2>&1; then
    fail "version mismatch (v999.0.0 vs ${VERSION}) was not rejected"
else
    pass "version mismatch is rejected (fail closed)"
fi

if sh "$CHECK" "v1.0.0foo" >/dev/null 2>&1; then
    fail "invalid tag was accepted by the version guard"
else
    pass "invalid tag is rejected by the version guard"
fi

# A package whose version disagrees with the tag must fail, using real metadata.
MISMATCH="$WORK/mismatch"
mkdir -p "$MISMATCH/src"
cat > "$MISMATCH/Cargo.toml" <<'EOF'
[package]
name = "pizen-lynx"
version = "0.1.0"
edition = "2021"
EOF
echo 'fn main() {}' > "$MISMATCH/src/main.rs"
if sh "$CHECK" "v9.9.9" --manifest-path "$MISMATCH/Cargo.toml" >/dev/null 2>&1; then
    fail "tag/manifest mismatch (v9.9.9 vs 0.1.0) was not rejected"
else
    pass "tag/manifest mismatch is rejected from resolved metadata"
fi

# A deliberately wrong binary must fail the binary check.
BIN="$WORK/lx"
cat > "$BIN" <<'EOF'
#!/bin/sh
echo "lx 0.0.0"
EOF
chmod +x "$BIN"
if sh "$CHECK" "v${VERSION}" --binary "$BIN" >/dev/null 2>&1; then
    fail "binary version mismatch was not rejected"
else
    pass "binary version mismatch is rejected"
fi

# --- 3. workspace hygiene: one product binary, no dead crates ----------------

echo "[workspace hygiene]"

BINS="$(cargo metadata --no-deps --format-version 1 --manifest-path "$ROOT/Cargo.toml" 2>/dev/null \
    | python3 -c '
import json, sys
data = json.load(sys.stdin)
names = sorted(t["name"] for p in data["packages"] for t in p["targets"] if "bin" in t["kind"])
print(" ".join(names))
')"
if [ "$BINS" = "lx" ]; then
    pass "the workspace declares exactly one binary: lx"
else
    fail "unexpected workspace binaries: '${BINS}' (expected 'lx')"
fi

if grep -rq 'pizen-lynx-common' "$ROOT/Cargo.toml" "$ROOT/crates"/*/Cargo.toml 2>/dev/null; then
    fail "the dead crate pizen-lynx-common is still referenced"
else
    pass "the dead crate pizen-lynx-common is not referenced"
fi

# --- 4. workflow wiring: prereleases cannot publish -------------------------

echo "[workflow wiring]"

job_block() { # job name -> prints the YAML block for that job
    awk -v job="$1" '
        $0 ~ "^  " job ":[[:space:]]*$" { inblock = 1; print; next }
        inblock && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { inblock = 0 }
        inblock { print }
    ' "$WORKFLOW"
}

if grep -q 'scripts/release/classify_tag.sh' "$WORKFLOW"; then
    pass "workflow classifies tags with the shared classifier"
else
    fail "workflow does not invoke scripts/release/classify_tag.sh"
fi

if grep -q 'scripts/release/check_versions.sh' "$WORKFLOW"; then
    pass "workflow enforces version consistency with the shared guard"
else
    fail "workflow does not invoke scripts/release/check_versions.sh"
fi

BUILD_BLOCK="$(job_block build)"
RELEASE_BLOCK="$(job_block release)"
PRERELEASE_BLOCK="$(job_block prerelease-validate)"

if printf '%s' "$BUILD_BLOCK" | grep -q "needs.classify.outputs.kind == 'stable'"; then
    pass "build job is gated on kind == stable"
else
    fail "build job is not gated on kind == stable"
fi

if printf '%s' "$RELEASE_BLOCK" | grep -q "needs.classify.outputs.kind == 'stable'"; then
    pass "release job is gated on kind == stable"
else
    fail "release job is not gated on kind == stable"
fi

release_creates="$(grep -c 'gh release create' "$WORKFLOW")"
if [ "$release_creates" -eq 1 ]; then
    pass "exactly one 'gh release create' in the workflow"
else
    fail "expected exactly one 'gh release create', found ${release_creates}"
fi

if printf '%s' "$RELEASE_BLOCK" | grep -q 'gh release create'; then
    pass "'gh release create' lives in the stable-gated release job"
else
    fail "'gh release create' is not in the stable-gated release job"
fi

if printf '%s' "$PRERELEASE_BLOCK" | grep -q 'gh release create'; then
    fail "prerelease job can create a release"
else
    pass "prerelease job cannot create a release"
fi

if printf '%s' "$PRERELEASE_BLOCK" | grep -q 'upload-artifact'; then
    fail "prerelease job uploads release artifacts"
else
    pass "prerelease job publishes no artifacts"
fi

if printf '%s' "$PRERELEASE_BLOCK" | grep -q 'kind == .prerelease.'; then
    pass "prerelease job is gated on kind == prerelease"
else
    fail "prerelease validation job is missing its explicit gate"
fi

# No Windows artifact is built: no Windows runner, no Windows Rust target.
if grep -Eqi 'windows-(latest|[0-9])|windows-msvc|windows-gnu|windows-arm64|windows-x86_64' "$WORKFLOW"; then
    fail "release workflow declares an (unsupported) Windows target"
else
    pass "release workflow builds no Windows target"
fi

# No macOS Intel artifact is built: upstream ONNX Runtime publishes no x86_64
# macOS build, so the target cannot be constructed and must not be declared.
if grep -Eqi 'x86_64-apple-darwin|macos-15-intel|macos-13' "$WORKFLOW"; then
    fail "release workflow declares an (unsupported) macOS Intel target"
else
    pass "release workflow builds no macOS Intel target"
fi

# --- 5. unsupported platform never releases ---------------------------------

echo "[unsupported target]"
FAKEBIN="$WORK/fakebin"; mkdir -p "$FAKEBIN"
cat > "$FAKEBIN/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo "Windows_NT" ;;
  -m) echo "x86_64" ;;
  *) echo "Windows_NT" ;;
esac
EOF
chmod +x "$FAKEBIN/uname"
UNSUP="$(PATH="$FAKEBIN:$PATH" LYNX_ALLOW_INSECURE=1 LYNX_BASE_URL="http://127.0.0.1:1" \
    sh "$INSTALLER" --dir "$WORK/bin-unsup" --version v1.0.0 2>&1)"
UNSUPSTATUS=$?
if [ "$UNSUPSTATUS" -ne 0 ] && [ ! -e "$WORK/bin-unsup/lx" ]; then
    pass "unsupported platform fails closed with nothing installed"
else
    fail "unsupported platform did not fail closed (status=$UNSUPSTATUS)"
fi
case "$UNSUP" in
    *unsupported*) pass "unsupported platform message is explicit" ;;
    *) fail "unsupported platform message missing: $UNSUP" ;;
esac

# --- 6. workflow-level dry run ----------------------------------------------

# Emulate the workflow gates for the tags that matter. The gate semantics are
# exactly those expressed in release.yml (asserted above): `build`/`release` run
# iff kind == stable; `prerelease-validate` runs iff kind == prerelease.
echo "[workflow dry run]"
for tag in v1.0.1 v1.0.1-rc.1 v1.0.1-alpha.1 v1.0.1-beta.1; do
    kind="$(sh "$CLASSIFY" "$tag" 2>/dev/null)"
    if [ "$kind" = "stable" ]; then
        plan="build -> release"
    elif [ "$kind" = "prerelease" ]; then
        plan="prerelease-validate only (STOP)"
    else
        plan="FAIL CLOSED"
    fi
    printf '  %-18s -> %-10s -> %s\n' "$tag" "$kind" "$plan"

    case "$tag" in
        v1.0.1)
            [ "$kind" = "stable" ] && [ "$plan" = "build -> release" ] \
                && pass "$tag takes the production release path" \
                || fail "$tag did not take the production release path"
            ;;
        *)
            [ "$kind" = "prerelease" ] && case "$plan" in
                *"prerelease-validate only"*) pass "$tag takes the validation-only path" ;;
                *) fail "$tag did not take the validation-only path" ;;
            esac || fail "$tag did not classify as prerelease"
            ;;
    esac
done

echo
if [ "$failures" -eq 0 ]; then
    echo "release_test: all tests passed"
    exit 0
fi
echo "release_test: ${failures} test(s) failed"
exit 1
