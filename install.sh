#!/bin/sh
# Lynx installer.
#
#   curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh
#
# Detects the platform, downloads the matching prebuilt release archive,
# verifies its SHA-256 checksum, and installs the `lx` binary atomically into a
# user-owned directory. No Rust/Node/Python/Docker runtime is required. Nothing
# outside the chosen install directory is modified. No telemetry.
#
# Environment:
#   LYNX_VERSION        Release tag to install (default: latest). e.g. v0.1.3
#   LYNX_INSTALL_DIR    Install directory (default: $HOME/.local/bin)
#   LYNX_REPO           GitHub owner/repo (default: PizenLabs/lynx)
#   LYNX_BASE_URL       Release download base (default: GitHub Releases for LYNX_REPO)
#   LYNX_DOWNLOAD_DIR   Target dir for --download-only (default: .)
#   LYNX_ALLOW_INSECURE Explicitly allow http:// for loopback testing only
#   LYNX_ALLOW_PRERELEASE
#                       Explicitly allow installing a prerelease tag (testing
#                       only; stable releases never need this)
#
# Usage:
#   sh install.sh [--help] [--version TAG] [--dir DIR] [--system]
#                 [--download-only[=DIR]] [--force]
#
# Exit status is non-zero on any failure; a failed install never replaces or
# removes an existing working `lx`.

set -eu

PROG="install.sh"
LYNX_REPO="${LYNX_REPO:-PizenLabs/lynx}"
LYNX_VERSION="${LYNX_VERSION:-latest}"
LYNX_INSTALL_DIR="${LYNX_INSTALL_DIR:-${HOME:-}/.local/bin}"
LYNX_BASE_URL="${LYNX_BASE_URL:-https://github.com/${LYNX_REPO}/releases/download}"
LYNX_DOWNLOAD_DIR="${LYNX_DOWNLOAD_DIR:-.}"
ALLOW_INSECURE="${LYNX_ALLOW_INSECURE:-0}"

MODE="install"   # install | download
SYSTEM=0
FORCE=0
TMPDIR_CREATED=""

err() { printf '%s: error: %s\n' "$PROG" "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }

cleanup() {
    if [ -n "$TMPDIR_CREATED" ] && [ -d "$TMPDIR_CREATED" ]; then
        rm -rf "$TMPDIR_CREATED"
    fi
}
trap cleanup EXIT HUP INT TERM

usage() {
    cat <<'EOF'
Lynx installer

Usage:
  sh install.sh [options]

Options:
  --version TAG        Release tag to install (e.g. v0.1.3). Default: latest.
  --dir DIR            Install directory. Default: $HOME/.local/bin.
  --system             Install to /usr/local/bin (explicit system install).
  --download-only[=DIR]
                       Download and verify only; do not install. DIR default: .
  --force              Reinstall even if the installed version is newer.
  --help               Show this help.

Environment:
  LYNX_VERSION, LYNX_INSTALL_DIR, LYNX_REPO, LYNX_BASE_URL, LYNX_DOWNLOAD_DIR

Examples:
  curl -fsSL https://raw.githubusercontent.com/PizenLabs/lynx/main/install.sh | sh
  LYNX_VERSION=v0.1.3 sh install.sh
  sh install.sh --dir "$HOME/bin"
  sh install.sh --download-only=/tmp
EOF
}

# --- argument parsing -------------------------------------------------------

while [ $# -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --version) shift; [ $# -gt 0 ] || err "--version needs a value"; LYNX_VERSION="$1" ;;
        --version=*) LYNX_VERSION="${1#--version=}" ;;
        --dir) shift; [ $# -gt 0 ] || err "--dir needs a value"; LYNX_INSTALL_DIR="$1" ;;
        --dir=*) LYNX_INSTALL_DIR="${1#--dir=}" ;;
        --system) SYSTEM=1; LYNX_INSTALL_DIR="/usr/local/bin" ;;
        --download-only) MODE="download" ;;
        --download-only=*) MODE="download"; LYNX_DOWNLOAD_DIR="${1#--download-only=}" ;;
        --force) FORCE=1 ;;
        --) shift; break ;;
        -*) err "unknown option: $1 (try --help)" ;;
        *) err "unexpected argument: $1 (try --help)" ;;
    esac
    shift
done

# --- install directory sanity -----------------------------------------------

# The default is "$HOME/.local/bin"; an unset HOME would silently become
# "/.local/bin". Fail clearly instead of writing somewhere unexpected.
if [ "$LYNX_INSTALL_DIR" = "/.local/bin" ]; then
    err "HOME is not set; specify an install directory with --dir DIR"
fi

# --- platform detection -----------------------------------------------------

case "$(uname -s)" in
    Darwin) OS="darwin" ;;
    Linux)  OS="linux" ;;
    *) err "unsupported operating system: $(uname -s) (supported: macOS, Linux)" ;;
esac

case "$(uname -m)" in
    arm64|aarch64) ARCH="arm64" ;;
    x86_64|amd64)  ARCH="x86_64" ;;
    *) err "unsupported architecture: $(uname -m) (supported: arm64, x86_64)" ;;
esac

PLATFORM="${OS}-${ARCH}"

# macOS Intel (x86_64) is not a supported target. The embedding engine is backed
# by ONNX Runtime, which publishes no x86_64 macOS build, so no prebuilt archive
# exists for it. Fail clearly instead of attempting a download that cannot
# succeed.
if [ "$PLATFORM" = "darwin-x86_64" ]; then
    err "unsupported platform: macOS Intel (x86_64)
  No prebuilt binary is published for it: the embedding engine requires ONNX
  Runtime, which has no x86_64 macOS build. Use an Apple Silicon Mac (arm64),
  or install from source: cargo install pizen-lynx"
fi

# --- integrity of the transport ---------------------------------------------

case "$LYNX_BASE_URL" in
    https://*) ;;
    http://*)
        # Explicit, loopback-only escape hatch for the distribution test suite.
        host="${LYNX_BASE_URL#http://}"
        host="${host%%/*}"
        case "$host" in
            \[*\]*) host="${host%%]*}]" ;;  # keep a bracketed IPv6 literal
            *)      host="${host%%:*}" ;;   # strip a :port suffix
        esac
        if [ "$ALLOW_INSECURE" != "1" ] || { [ "$host" != "127.0.0.1" ] && [ "$host" != "localhost" ] && [ "$host" != "[::1]" ]; }; then
            err "refusing non-HTTPS download base: $LYNX_BASE_URL (set LYNX_ALLOW_INSECURE=1 with a loopback host only for testing)"
        fi
        CURL_PROTO="=http,https"
        ;;
    *) err "download base must be an https:// URL, got: $LYNX_BASE_URL" ;;
esac
: "${CURL_PROTO:=https}"

for tool in curl tar; do
    command -v "$tool" >/dev/null 2>&1 || err "required tool not found: $tool"
done

if command -v sha256sum >/dev/null 2>&1; then
    SHA_CMD="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    SHA_CMD="shasum -a 256"
else
    err "no SHA-256 tool found (need sha256sum or shasum)"
fi

# --- HTTP helpers -----------------------------------------------------------

fetch() { # url dest
    curl -fsSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" --tlsv1.2 -o "$2" "$1" \
        || return 1
}

resolve_latest() {
    # Follow the /releases/latest redirect to its /tag/<tag> URL. No API, no
    # authentication, no JSON parsing, no rate limit.
    url="https://github.com/${LYNX_REPO}/releases/latest"
    effective=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$url" 2>/dev/null) \
        || err "could not resolve the latest release for ${LYNX_REPO} (network or 404)"
    tag="${effective##*/tag/}"
    case "$tag" in
        ""|"$effective") err "could not parse the latest release tag from: $effective" ;;
    esac
    printf '%s' "$tag"
}

# --- version helpers --------------------------------------------------------

normalize_tag() { # accept 1.2.3 or v1.2.3 -> v1.2.3
    case "$1" in
        v*) printf '%s' "$1" ;;
        *) printf 'v%s' "$1" ;;
    esac
}

# version_gt A B -> exit 0 iff A is strictly newer than B (major.minor.patch).
version_gt() {
    awk -v a="$1" -v b="$2" 'BEGIN{
        gsub(/^v/,"",a); gsub(/^v/,"",b);
        sub(/-.*/,"",a); sub(/-.*/,"",b);
        n=split(a,A,"."); m=split(b,B,".");
        for (i=1;i<=3;i++){ ai=(i<=n?A[i]+0:0); bi=(i<=m?B[i]+0:0);
            if (ai>bi) exit 0; if (ai<bi) exit 1; }
        exit 1;
    }'
}

# --- resolve release --------------------------------------------------------

if [ "$LYNX_VERSION" = "latest" ]; then
    TAG=$(resolve_latest)
else
    TAG=$(normalize_tag "$LYNX_VERSION")
fi
[ -n "$TAG" ] || err "could not determine a release tag to install"

# --- stable-only guard ------------------------------------------------------
# The installer consumes only stable production releases. `latest` resolves via
# GitHub's /releases/latest, which the GitHub API defines as the latest
# non-prerelease, non-draft release, so it can never pick `-rc`/`-alpha`/`-beta`
# artifacts. An explicitly requested tag is validated here so a prerelease can
# never be installed by accident.
stable_re='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
prerelease_re='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-[0-9A-Za-z.-]+$'
if ! printf '%s' "$TAG" | grep -Eq "$stable_re"; then
    if [ "${LYNX_ALLOW_PRERELEASE:-0}" = "1" ] && printf '%s' "$TAG" | grep -Eq "$prerelease_re"; then
        info "  warning: installing prerelease ${TAG} (LYNX_ALLOW_PRERELEASE=1)"
    else
        err "refusing to install non-stable release: ${TAG}
  Supported releases are stable vMAJOR.MINOR.PATCH tags only.
  Prerelease tags (-rc, -alpha, -beta, -dev, ...) do not update the stable
  release surface. Set LYNX_ALLOW_PRERELEASE=1 only to install one for testing."
    fi
fi

ASSET="lynx-${TAG}-${PLATFORM}.tar.gz"
RELEASE_BASE="${LYNX_BASE_URL}/${TAG}"
ASSET_URL="${RELEASE_BASE}/${ASSET}"
SUMS_URL="${RELEASE_BASE}/SHA256SUMS"

# --- download ---------------------------------------------------------------

STAGE=$(mktemp -d "${TMPDIR:-/tmp}/lynx-install.XXXXXX") \
    || err "could not create a temporary directory"
TMPDIR_CREATED="$STAGE"
ARCHIVE="${STAGE}/${ASSET}"
SUMS="${STAGE}/SHA256SUMS"

info "Lynx ${TAG} (${PLATFORM})"
info "  downloading ${ASSET_URL}"
fetch "$ASSET_URL" "$ARCHIVE" \
    || err "download failed: ${ASSET_URL} (network error, missing release, or HTTP error)"

fetch "$SUMS_URL" "$SUMS" \
    || err "could not download SHA256SUMS from ${SUMS_URL}"

# --- verify -----------------------------------------------------------------

EXPECTED=$(awk -v name="$ASSET" '$2 == name || $2 == "*"name { print $1; exit }' "$SUMS")
if [ -z "$EXPECTED" ]; then
    # SHA256SUMS may list bare filenames without a leading marker.
    EXPECTED=$(awk -v name="$ASSET" '{ f=$NF; sub(/^\*/,"",f); if (f == name) { print $1; exit } }' "$SUMS")
fi
[ -n "$EXPECTED" ] || err "no checksum entry for ${ASSET} in SHA256SUMS"

ACTUAL=$($SHA_CMD "$ARCHIVE" | awk '{ print $1 }')
if [ "$EXPECTED" != "$ACTUAL" ]; then
    err "checksum mismatch for ${ASSET}
  expected: ${EXPECTED}
  actual:   ${ACTUAL}
  refusing to install a corrupted or tampered archive."
fi
info "  checksum verified"

# --- download-only mode -----------------------------------------------------

if [ "$MODE" = "download" ]; then
    mkdir -p "$LYNX_DOWNLOAD_DIR" || err "could not create download dir: $LYNX_DOWNLOAD_DIR"
    DEST_DIR=$(cd "$LYNX_DOWNLOAD_DIR" && pwd)
    cp -p "$ARCHIVE" "${DEST_DIR}/${ASSET}" || err "could not write into ${DEST_DIR}"
    cp -p "$SUMS" "${DEST_DIR}/SHA256SUMS" || err "could not write into ${DEST_DIR}"
    info "  wrote ${DEST_DIR}/${ASSET}"
    info "  wrote ${DEST_DIR}/SHA256SUMS"
    info "download-only: nothing was installed."
    exit 0
fi

# --- unpack -----------------------------------------------------------------

mkdir -p "${STAGE}/unpack"
tar -xzf "$ARCHIVE" -C "${STAGE}/unpack" \
    || err "could not unpack ${ASSET} (corrupt archive?)"

BIN_SRC="${STAGE}/unpack/lx"
[ -f "$BIN_SRC" ] || err "the archive did not contain an 'lx' binary"
[ -x "$BIN_SRC" ] || chmod 0755 "$BIN_SRC" 2>/dev/null || true

# --- pre-flight existing install -------------------------------------------

BIN_DST="${LYNX_INSTALL_DIR}/lx"
if [ -x "$BIN_DST" ]; then
    EXISTING=$("$BIN_DST" --version 2>/dev/null | awk '{ print $NF }') || EXISTING=""
    if [ -n "$EXISTING" ] && [ "$FORCE" -ne 1 ]; then
        if version_gt "$EXISTING" "$TAG"; then
            err "an installed lx ${EXISTING} is newer than the requested ${TAG}; refusing to downgrade (use --force to override)"
        fi
    fi
fi

# --- atomic install ---------------------------------------------------------

if [ "$SYSTEM" -eq 1 ]; then
    if [ ! -w "$LYNX_INSTALL_DIR" ]; then
        err "system install requested but ${LYNX_INSTALL_DIR} is not writable; re-run with sudo, or omit --system for a user install"
    fi
fi

mkdir -p "$LYNX_INSTALL_DIR" \
    || err "could not create install directory: ${LYNX_INSTALL_DIR} (permission denied?)"
[ -d "$LYNX_INSTALL_DIR" ] && [ -w "$LYNX_INSTALL_DIR" ] \
    || err "install directory is not writable: ${LYNX_INSTALL_DIR}"

# Same-directory temp file + rename: atomic, and never leaves a partial binary
# in place. The previous binary stays usable until the rename succeeds.
INSTALLING=$(mktemp "${LYNX_INSTALL_DIR}/.lx-install.XXXXXX") \
    || err "could not create a temporary file in ${LYNX_INSTALL_DIR}"
cat "$BIN_SRC" > "$INSTALLING" || { rm -f "$INSTALLING"; err "could not stage the binary"; }
chmod 0755 "$INSTALLING" || { rm -f "$INSTALLING"; err "could not set executable permissions"; }

if ! mv -f "$INSTALLING" "$BIN_DST"; then
    rm -f "$INSTALLING"
    err "could not replace ${BIN_DST} (the previous binary is untouched)"
fi

# --- verify the installed binary -------------------------------------------

INSTALLED_VERSION=$("$BIN_DST" --version 2>/dev/null) \
    || err "the installed binary failed to run: ${BIN_DST} --version"
case "$INSTALLED_VERSION" in
    *"${TAG#v}"*) ;;
    *) err "installed version mismatch: expected ${TAG#v}, got: ${INSTALLED_VERSION}" ;;
esac

info "  installed $BIN_DST ($INSTALLED_VERSION)"

# --- PATH guidance ----------------------------------------------------------

case ":${PATH}:" in
    *":${LYNX_INSTALL_DIR}:"*) ;;
    *)
        info ""
        info "Add Lynx to your PATH:"
        info "  export PATH=\"${LYNX_INSTALL_DIR}:\$PATH\""
        info "Then restart your shell (or source your shell's rc file). No shell"
        info "startup file was modified by this installer."
        ;;
esac

info ""
info "Lynx installed. Verify with:"
info "  lx --version"
info "  lx doctor"
