#!/usr/bin/env bash
# Clean-environment `cargo install` proof.
#
# Packages every workspace crate with `cargo package`, assembles a local Cargo
# registry from those archives plus the third-party dependency closure, replaces
# crates.io with it, and installs the public product package (`pizen-lynx`) with
# a fresh CARGO_HOME — never from the source checkout. It then runs the
# installed `lx`.
#
# This is the registry analogue of `clean_machine_test.sh`. crates.io is
# simulated with Cargo's built-in `local-registry` source, so the install is a
# single-registry consumption of exactly the archives `cargo package` produced.
# It needs network access for `cargo fetch` and a C toolchain (rusqlite /
# onnxruntime build), but no publishing credentials.
#
# Usage: tests/distribution/cargo_install_test.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
GEN="$HERE/make_registry.py"

failures=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 2; }; }
need cargo; need python3; need tar

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lynx-cargo-install.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
[ -n "$VERSION" ] || { echo "could not read the workspace version" >&2; exit 2; }
echo "cargo_install_test: version=${VERSION}"

# A published package requires its internal dependencies to already exist on the
# registry, so package them here with local [patch] overrides. The patch only
# affects dependency *resolution*; each emitted Cargo.toml still points at the
# registry versions, so the archives are exactly what crates.io would receive.
PATCH_CONFIGS=()
for n in core embed mcp parser protocol storage; do
    PATCH_CONFIGS+=(--config "patch.crates-io.pizen-lynx-${n}.path=\"${ROOT}/crates/lynx-${n}\"")
done

REG="$WORK/registry"
mkdir -p "$REG"
PACKAGES=(
    pizen-lynx-protocol pizen-lynx-parser pizen-lynx-embed
    pizen-lynx-storage pizen-lynx-core pizen-lynx-mcp pizen-lynx
)
packaged=0
for p in "${PACKAGES[@]}"; do
    if cargo package --no-verify --allow-dirty -p "$p" "${PATCH_CONFIGS[@]}" \
        >/dev/null 2>"$WORK/package-${p}.log"; then
        src="$ROOT/target/package/${p}-${VERSION}.crate"
        if [ -f "$src" ]; then
            cp "$src" "$REG/"
            packaged=$((packaged + 1))
        else
            fail "package archive missing: $src"
        fi
    else
        fail "cargo package ${p}"
        sed 's/^/        /' "$WORK/package-${p}.log"
    fi
done
[ "$packaged" -eq "${#PACKAGES[@]}" ] && pass "packaged all ${#PACKAGES[@]} workspace crates"

echo "[fetch dependency closure]"
CARGO_HOME="$WORK/cargo-home"
mkdir -p "$CARGO_HOME"
if CARGO_HOME="$CARGO_HOME" cargo fetch --manifest-path "$ROOT/Cargo.toml" >"$WORK/fetch.log" 2>&1; then
    pass "fetched the third-party dependency closure"
else
    fail "cargo fetch"
    tail -10 "$WORK/fetch.log" | sed 's/^/        /'
fi

# Assemble the single-registry dependency set: our archives + the closure.
CACHE="$CARGO_HOME/registry/cache"
find "$CACHE" -name '*.crate' -exec cp -n {} "$REG/" \; 2>/dev/null || true
count="$(find "$REG" -maxdepth 1 -name '*.crate' | wc -l | tr -d ' ')"
echo "  registry contains ${count} crate archives"

# Build the index (crates.io replacement: same-registry deps omit `registry`).
if python3 "$GEN" --crates-dir "$REG" --out "$REG/index" --index-url "" >"$WORK/gen.log" 2>&1; then
    pass "generated the local registry index"
else
    fail "generating the registry index"; sed 's/^/        /' "$WORK/gen.log"
    echo; echo "cargo_install_test: ${failures} check(s) failed"; exit 1
fi

cat > "$CARGO_HOME/config.toml" <<EOF
[source.crates-io]
replace-with = "lynx-local"

[source.lynx-local]
local-registry = "$REG"
EOF

echo "[cargo install]"
if CARGO_HOME="$CARGO_HOME" \
    CARGO_TARGET_DIR="${LYNX_CARGO_INSTALL_TARGET_DIR:-$WORK/target}" \
    cargo install --offline pizen-lynx --version "$VERSION" --root "$WORK/root" \
    >"$WORK/install.log" 2>&1; then
    pass "cargo install pizen-lynx ${VERSION} from a clean registry"
else
    fail "cargo install pizen-lynx ${VERSION} from a clean registry"
    tail -30 "$WORK/install.log" | sed 's/^/        /'
fi

LX="$WORK/root/bin/lx"
if [ ! -x "$LX" ]; then
    fail "installed binary missing: $LX"
    echo; echo "cargo_install_test: ${failures} check(s) failed"; exit 1
fi

echo "[binary]"
if [ "$("$LX" --version)" = "lx ${VERSION}" ]; then
    pass "installed binary reports the release version"
else
    fail "installed binary version mismatch: $("$LX" --version)"
fi

echo "[single product binary]"
BINS="$(cd "$WORK/root/bin" && ls -1 | sort | tr '\n' ' ')"
if [ "$BINS" = "lx " ]; then
    pass "pizen-lynx installs exactly one binary (lx)"
else
    fail "unexpected installed binaries: ${BINS}"
fi

echo "[doctor]"
DOCTOR_OUT="$(cd "$WORK" && "$LX" doctor 2>&1)"
case "$DOCTOR_OUT" in
    *"Lynx Doctor"*) pass "installed binary runs \`lx doctor\`" ;;
    *) fail "doctor failed: $DOCTOR_OUT" ;;
esac

echo "[mcp --check]"
MCP_OUT="$(cd "$WORK" && "$LX" mcp --check 2>&1)"
case "$MCP_OUT" in
    *"Status       OK"*) pass "installed binary runs \`lx mcp --check\`" ;;
    *) fail "mcp --check failed: $MCP_OUT" ;;
esac

echo "[hermeticity]"
case "$(cd "$(dirname "$LX")" && pwd)/$(basename "$LX")" in
    "$ROOT"/*) fail "installed binary resolved back into the source checkout" ;;
    *) pass "installed binary is outside the source checkout" ;;
esac

echo
if [ "$failures" -eq 0 ]; then
    echo "cargo_install_test: all checks passed"
    exit 0
fi
echo "cargo_install_test: ${failures} check(s) failed"
exit 1
