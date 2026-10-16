#!/usr/bin/env bash
# Distribution / installer test suite.
#
# Exercises `install.sh` against a local release server (a synthetic release
# tree served over loopback HTTP). Covers fresh install, repeat install,
# upgrade, downgrade refusal, checksum failure, missing artifact, unsupported
# platform, atomic replacement, previous-install preservation, PATH guidance,
# and download-only mode.
#
# Requirements: bash, curl, tar, shasum/sha256sum, python3 (test server only).
# The installer itself needs none of python/node/rust.
#
# Usage: tests/distribution/install_test.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALLER="$ROOT/install.sh"

failures=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lynx-disttest.XXXXXX")"
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 2; }; }
need curl; need tar; need python3
if command -v sha256sum >/dev/null 2>&1; then SHA="sha256sum"; else SHA="shasum -a 256"; fi

# Host platform, mirrored from install.sh.
case "$(uname -s)" in Darwin) OS=darwin ;; Linux) OS=linux ;; *) echo "unsupported host" >&2; exit 2 ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=arm64 ;; x86_64|amd64) ARCH=x86_64 ;; *) echo "unsupported host arch" >&2; exit 2 ;; esac
PLATFORM="${OS}-${ARCH}"

# --- synthetic release tree -------------------------------------------------

RELEASES="$WORK/releases"

# make_release TAG -> writes releases/<TAG>/ with the archive + SHA256SUMS
make_release() {
    local tag="$1"
    local dir="$RELEASES/$tag"
    local version="${tag#v}"
    mkdir -p "$dir/stage"
    cat > "$dir/stage/lx" <<EOF
#!/bin/sh
if [ "\$1" = "--version" ]; then echo "lx $version"; exit 0; fi
echo "stub lx $version \$*"
EOF
    chmod 0755 "$dir/stage/lx"
    local asset="lynx-${tag}-${PLATFORM}.tar.gz"
    ( cd "$dir/stage" && tar -czf "$dir/$asset" lx )
    ( cd "$dir" && $SHA "$asset" > SHA256SUMS )
}

# --- local HTTP server ------------------------------------------------------

PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
BASE="http://127.0.0.1:${PORT}"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$RELEASES" >/dev/null 2>&1 &
SERVER_PID=$!
# Wait for the server to accept connections.
for _ in $(seq 1 50); do
    curl -fsS -o /dev/null "$BASE/" 2>/dev/null && break
    sleep 0.1
done

# --- helpers ----------------------------------------------------------------

run_install() {
    # run_install <bindir> <args...> ; captures output in $OUT, status in $STATUS
    local bindir="$1"; shift
    OUT="$(LYNX_ALLOW_INSECURE=1 LYNX_BASE_URL="$BASE" sh "$INSTALLER" --dir "$bindir" "$@" 2>&1)"
    STATUS=$?
}

# --- tests ------------------------------------------------------------------

echo "install_test: platform=${PLATFORM} base=${BASE}"

make_release v1.0.1
make_release v1.0.2

# 1. fresh install
echo "[fresh install]"
BIN="$WORK/bin1"; run_install "$BIN" --version v1.0.1
if [ "$STATUS" -eq 0 ] && [ -x "$BIN/lx" ] && [ "$("$BIN/lx" --version)" = "lx 1.0.1" ]; then
    pass "installs and runs the requested version"
else
    fail "fresh install (status=$STATUS)"; echo "$OUT"
fi
case "$OUT" in *"Add Lynx to your PATH"*) pass "prints PATH guidance when not on PATH" ;; *) fail "PATH guidance missing" ;; esac

# 2. idempotent repeat install
echo "[repeat install]"
run_install "$BIN" --version v1.0.1
if [ "$STATUS" -eq 0 ] && [ "$("$BIN/lx" --version)" = "lx 1.0.1" ]; then
    pass "repeat install is idempotent"
else
    fail "repeat install (status=$STATUS)"
fi

# 3. upgrade
echo "[upgrade]"
run_install "$BIN" --version v1.0.2
if [ "$STATUS" -eq 0 ] && [ "$("$BIN/lx" --version)" = "lx 1.0.2" ]; then
    pass "upgrades to a newer version"
else
    fail "upgrade (status=$STATUS)"
fi

# 4. downgrade refusal preserves the existing binary
echo "[downgrade refusal]"
run_install "$BIN" --version v1.0.1
if [ "$STATUS" -ne 0 ] && [ "$("$BIN/lx" --version)" = "lx 1.0.2" ]; then
    pass "refuses to downgrade and preserves the existing binary"
else
    fail "downgrade refusal (status=$STATUS)"
fi
case "$OUT" in *"refusing to downgrade"*) pass "downgrade message is explicit" ;; *) fail "downgrade message missing" ;; esac

# 5. checksum mismatch fails and preserves the existing binary
echo "[checksum failure]"
BADROOT="$WORK/badreleases"
mkdir -p "$BADROOT/v1.0.8"
cp "$RELEASES/v1.0.2/lynx-v1.0.2-${PLATFORM}.tar.gz" "$BADROOT/v1.0.8/lynx-v1.0.8-${PLATFORM}.tar.gz"
printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "lynx-v1.0.8-${PLATFORM}.tar.gz" > "$BADROOT/v1.0.8/SHA256SUMS"
BADPORT="$(python3 - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
)"
python3 -m http.server "$BADPORT" --bind 127.0.0.1 --directory "$BADROOT" >/dev/null 2>&1 &
BADPID=$!
for _ in $(seq 1 50); do curl -fsS -o /dev/null "http://127.0.0.1:${BADPORT}/" 2>/dev/null && break; sleep 0.1; done
BADOUT="$(LYNX_ALLOW_INSECURE=1 LYNX_BASE_URL="http://127.0.0.1:${BADPORT}" sh "$INSTALLER" --dir "$BIN" --version v1.0.8 2>&1)"
BADSTATUS=$?
kill "$BADPID" 2>/dev/null
wait "$BADPID" 2>/dev/null || true
if [ "$BADSTATUS" -ne 0 ] && [ "$("$BIN/lx" --version)" = "lx 1.0.2" ]; then
    pass "checksum mismatch fails and preserves the existing binary"
else
    fail "checksum failure (status=$BADSTATUS)"; echo "$BADOUT"
fi
case "$BADOUT" in *"checksum mismatch"*) pass "checksum failure message is explicit" ;; *) fail "checksum message missing" ;; esac

# 6. missing artifact fails clearly
echo "[missing artifact]"
run_install "$WORK/bin-missing" --version v9.9.9
if [ "$STATUS" -ne 0 ] && [ ! -e "$WORK/bin-missing/lx" ]; then
    pass "missing release fails without installing anything"
else
    fail "missing artifact (status=$STATUS)"
fi

# 7. unsupported platform fails clearly
echo "[unsupported platform]"
FAKEBIN="$WORK/fakebin"; mkdir -p "$FAKEBIN"
cat > "$FAKEBIN/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo "Plan9" ;;
  -m) echo "mips" ;;
  *) echo "Plan9" ;;
esac
EOF
chmod +x "$FAKEBIN/uname"
UNSUP="$(PATH="$FAKEBIN:$PATH" LYNX_ALLOW_INSECURE=1 LYNX_BASE_URL="$BASE" sh "$INSTALLER" --dir "$WORK/bin-unsup" --version v1.0.1 2>&1)"
UNSUPSTATUS=$?
if [ "$UNSUPSTATUS" -ne 0 ]; then pass "unsupported platform exits non-zero"; else fail "unsupported platform did not fail"; fi
case "$UNSUP" in *"unsupported"*) pass "unsupported platform message is explicit" ;; *) fail "unsupported message missing: $UNSUP" ;; esac

# 8. no partial/temp files left behind
echo "[atomic replacement]"
if ls "$BIN"/.lx-install.* >/dev/null 2>&1; then
    fail "leftover temporary files in install dir"
else
    pass "no partial files remain after installs"
fi

# 9. non-HTTPS base rejected unless explicitly allowed
echo "[http rejected by default]"
HTTPOUT="$(LYNX_BASE_URL="$BASE" sh "$INSTALLER" --dir "$WORK/bin-http" --version v1.0.1 2>&1)"
HTTPSTATUS=$?
if [ "$HTTPSTATUS" -ne 0 ]; then pass "non-HTTPS base rejected without the override"; else fail "http base was accepted"; fi

# 10. download-only writes artifacts and installs nothing
echo "[download-only]"
DL="$WORK/downloads"; mkdir -p "$DL"
run_install "$BIN" --version v1.0.1 --download-only="$DL"
if [ "$STATUS" -eq 0 ] && [ -f "$DL/lynx-v1.0.1-${PLATFORM}.tar.gz" ] && [ -f "$DL/SHA256SUMS" ]; then
    pass "download-only writes the verified archive and sums"
else
    fail "download-only (status=$STATUS)"; echo "$OUT"
fi
case "$OUT" in *"nothing was installed"*) pass "download-only reports no install" ;; *) fail "download-only message missing" ;; esac

# 11. --help exits 0
echo "[help]"
HELP="$(sh "$INSTALLER" --help 2>&1)"; HELPSTATUS=$?
if [ "$HELPSTATUS" -eq 0 ]; then pass "--help exits 0"; else fail "--help exit status $HELPSTATUS"; fi
case "$HELP" in *"Usage:"*) pass "--help prints usage" ;; *) fail "--help usage missing" ;; esac

echo
if [ "$failures" -eq 0 ]; then
    echo "install_test: all tests passed"
    exit 0
else
    echo "install_test: ${failures} test(s) failed"
    exit 1
fi
