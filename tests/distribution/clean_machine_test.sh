#!/usr/bin/env bash
# Clean-machine distribution validation.
#
# Simulates the documented install path end to end using the real release
# binary: package it as a release, serve it locally, install it with
# `install.sh` into a fresh prefix, then run the first-repository workflow and
# an independent MCP consumer against the *installed* binary outside any
# development checkout.
#
# It needs the embedding model available (downloaded once into
# $HOME/.cache/lynx/fastembed) because `lx index` embeds; everything else is
# hermetic. It never touches the repository's own `.lynx`.
#
# Usage: tests/distribution/clean_machine_test.sh [path/to/lx]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/release/lx}"
INSTALLER="$ROOT/install.sh"
MCP_CLIENT="$(dirname "$0")/mcp_client.py"

failures=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

[ -x "$BIN" ] || { echo "release binary not found: $BIN (run: cargo build --release -p pizen-lynx)" >&2; exit 2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lynx-clean.XXXXXX")"
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

case "$(uname -s)" in Darwin) OS=darwin ;; Linux) OS=linux ;; *) echo unsupported host >&2; exit 2 ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=arm64 ;; x86_64|amd64) ARCH=x86_64 ;; *) echo unsupported host >&2; exit 2 ;; esac
PLATFORM="${OS}-${ARCH}"

VERSION="$("$BIN" --version | awk '{print $2}')"
TAG="v${VERSION}"
ASSET="lynx-${TAG}-${PLATFORM}.tar.gz"

echo "clean_machine_test: binary=$BIN tag=$TAG platform=$PLATFORM"

# --- 1. package the real binary as a release --------------------------------

RELEASES="$WORK/releases"
mkdir -p "$RELEASES/$TAG/stage"
cp "$BIN" "$RELEASES/$TAG/stage/lx"
chmod 0755 "$RELEASES/$TAG/stage/lx"
( cd "$RELEASES/$TAG/stage" && tar -czf "../$ASSET" lx )
if command -v sha256sum >/dev/null 2>&1; then SHA="sha256sum"; else SHA="shasum -a 256"; fi
( cd "$RELEASES/$TAG" && $SHA "$ASSET" > SHA256SUMS )

PORT="$(python3 - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
)"
BASE="http://127.0.0.1:${PORT}"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$RELEASES" >/dev/null 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -fsS -o /dev/null "$BASE/" 2>/dev/null && break; sleep 0.1; done

# --- 2. install through the real installer ----------------------------------

PREFIX="$WORK/prefix"
echo "[install]"
INSTALL_OUT="$(LYNX_ALLOW_INSECURE=1 LYNX_BASE_URL="$BASE" sh "$INSTALLER" --dir "$PREFIX" --version "$TAG" 2>&1)"
INSTALL_STATUS=$?
LX="$PREFIX/lx"
if [ "$INSTALL_STATUS" -eq 0 ] && [ -x "$LX" ]; then
    pass "installer installed the real binary"
else
    fail "installer failed (status=$INSTALL_STATUS)"; echo "$INSTALL_OUT"
fi

# --- 3. binary works outside the checkout -----------------------------------

echo "[binary]"
if [ "$("$LX" --version)" = "lx $VERSION" ]; then
    pass "installed binary reports its version"
else
    fail "installed binary version mismatch: $("$LX" --version)"
fi

# --- 4. arbitrary repository workflow ---------------------------------------

REPO="$WORK/arbitrary-repo"
mkdir -p "$REPO/auth" "$REPO/web"
cat > "$REPO/auth/token.go" <<'EOF'
package auth

type Token struct {
    Subject string
    Expiry  int64
}

func ValidateToken(tok string) bool {
    return len(tok) > 0 && tok[0] != 'x'
}
EOF
cat > "$REPO/web/session.ts" <<'EOF'
export function refreshSession(userId: string): string {
    return "session-" + userId;
}
EOF
cat > "$REPO/auth/handler.rs" <<'EOF'
pub struct AuthHandler;

pub fn handle_login(user: &str) -> bool {
    !user.is_empty()
}
EOF

echo "[doctor before index]"
DOCTOR_PRE="$(cd "$REPO" && "$LX" doctor 2>&1)"
case "$DOCTOR_PRE" in *"NOT READY"*) pass "doctor reports NOT READY before indexing" ;; *) fail "doctor pre-index: $DOCTOR_PRE" ;; esac

echo "[index]"
if (cd "$REPO" && "$LX" index >/dev/null 2>&1) && [ -d "$REPO/.lynx" ]; then
    pass "lx index built .lynx in an arbitrary repository"
else
    fail "lx index failed"
fi

echo "[status]"
STATUS_OUT="$(cd "$REPO" && "$LX" status 2>&1)"
case "$STATUS_OUT" in *"freshness   fresh"*) pass "status reports a fresh index" ;; *) fail "status: $STATUS_OUT" ;; esac

echo "[search]"
SEARCH_OUT="$(cd "$REPO" && "$LX" search "validate token" 2>&1)"
case "$SEARCH_OUT" in *ValidateToken*) pass "search surfaces the expected symbol" ;; *) fail "search: $SEARCH_OUT" ;; esac

echo "[resolve]"
RESOLVE_OUT="$(cd "$REPO" && "$LX" resolve ValidateToken 2>&1)"
case "$RESOLVE_OUT" in *ValidateToken*) pass "resolve returns coordinates" ;; *) fail "resolve: $RESOLVE_OUT" ;; esac

echo "[context]"
CONTEXT_OUT="$(cd "$REPO" && "$LX" context "token validation" 2>&1)"
case "$CONTEXT_OUT" in *estimated_tokens*) pass "context returns a bounded package" ;; *) fail "context: $CONTEXT_OUT" ;; esac

echo "[doctor after index]"
DOCTOR_POST="$(cd "$REPO" && "$LX" doctor 2>&1)"
case "$DOCTOR_POST" in *"Status       READY"*) pass "doctor reports READY after indexing" ;; *) fail "doctor post-index: $DOCTOR_POST" ;; esac

# --- 5. independent MCP consumer --------------------------------------------

echo "[mcp consumer]"
if MCP_OUT="$(python3 "$MCP_CLIENT" "$LX" "$REPO" 2>&1)"; then
    pass "independent MCP consumer completed search/resolve/context/index_status"
    printf '%s\n' "$MCP_OUT"
else
    fail "MCP consumer failed:"; printf '%s\n' "$MCP_OUT"
fi

# --- 6. no development-checkout assumptions ---------------------------------

echo "[hermeticity]"
# The installed binary must not resolve back into the source checkout.
case "$(cd "$(dirname "$LX")" && pwd)/$(basename "$LX")" in
    "$ROOT"/*) fail "installed binary resolved back into the source checkout" ;;
    *) pass "installed binary is outside the source checkout" ;;
esac

echo
if [ "$failures" -eq 0 ]; then
    echo "clean_machine_test: all checks passed"
    exit 0
else
    echo "clean_machine_test: ${failures} check(s) failed"
    exit 1
fi
