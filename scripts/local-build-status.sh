#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${LIMUX_LOCAL_PREFIX:-$HOME/.local}"

CLI_DEST="$PREFIX/bin/limux"
HOST_DIR="$PREFIX/libexec/limux"
HOST_WRAPPER="$HOST_DIR/limux-host"
HOST_BIN_DEST="$HOST_DIR/limux-host.bin"
LIB_DEST="$PREFIX/lib/limux/libghostty.so"
BUILD_INFO_DEST="$PREFIX/share/limux/local-build.txt"

failures=0

fail() {
    echo "FAIL: $*" >&2
    failures=$((failures + 1))
}

warn() {
    echo "WARN: $*" >&2
}

pass() {
    echo "OK: $*"
}

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        fail "$1 is required"
        return 1
    fi
}

realpath_of() {
    readlink -f "$1" 2>/dev/null || true
}

need_cmd awk
need_cmd git
need_cmd grep
need_cmd ldd
need_cmd readlink

echo "Limux local build status"
echo "  repo:   $ROOT_DIR"
echo "  prefix: $PREFIX"

ACTIVE_LIMUX="$(command -v limux 2>/dev/null || true)"
if [ -z "$ACTIVE_LIMUX" ]; then
    fail "limux is not on PATH"
else
    ACTIVE_REAL="$(realpath_of "$ACTIVE_LIMUX")"
    EXPECTED_REAL="$(realpath_of "$CLI_DEST")"
    if [ -n "$EXPECTED_REAL" ] && [ "$ACTIVE_REAL" = "$EXPECTED_REAL" ]; then
        pass "active limux resolves to $CLI_DEST"
    else
        fail "active limux is $ACTIVE_LIMUX, expected $CLI_DEST"
    fi
fi

if [ -x "$CLI_DEST" ]; then
    pass "CLI exists at $CLI_DEST"
    if "$CLI_DEST" --help 2>&1 | grep -q "limux CLI"; then
        pass "CLI help identifies the Limux CLI entrypoint"
    else
        fail "$CLI_DEST does not look like the Limux CLI entrypoint"
    fi
else
    fail "CLI is missing or not executable at $CLI_DEST"
fi

if [ -x "$HOST_WRAPPER" ]; then
    pass "host wrapper exists at $HOST_WRAPPER"
else
    fail "host wrapper is missing or not executable at $HOST_WRAPPER"
fi

if [ -x "$HOST_BIN_DEST" ]; then
    pass "host binary exists at $HOST_BIN_DEST"
else
    fail "host binary is missing or not executable at $HOST_BIN_DEST"
fi

if [ -f "$LIB_DEST" ]; then
    pass "libghostty.so exists at $LIB_DEST"
else
    fail "libghostty.so is missing at $LIB_DEST"
fi

if [ -x "$HOST_BIN_DEST" ] && [ -f "$LIB_DEST" ]; then
    RESOLVED_LIB="$(
        LD_LIBRARY_PATH="$PREFIX/lib/limux${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
            ldd "$HOST_BIN_DEST" \
            | awk '/libghostty\.so/ {print $3; exit}'
    )"
    if [ -z "$RESOLVED_LIB" ]; then
        fail "host binary does not resolve libghostty.so"
    elif [ "$(realpath_of "$RESOLVED_LIB")" = "$(realpath_of "$LIB_DEST")" ]; then
        pass "host resolves libghostty.so from the local prefix"
    else
        fail "host resolves libghostty.so to $RESOLVED_LIB, expected $LIB_DEST"
    fi
fi

if [ -x "$HOST_WRAPPER" ]; then
    if "$HOST_WRAPPER" --version >/dev/null 2>&1; then
        pass "host wrapper executes"
    else
        fail "host wrapper failed to execute"
    fi
fi

if [ -f "$BUILD_INFO_DEST" ]; then
    echo "Build info:"
    sed 's/^/  /' "$BUILD_INFO_DEST"

    INSTALLED_HEAD="$(awk -F= '/^git_head=/ {print $2; exit}' "$BUILD_INFO_DEST")"
    CURRENT_HEAD="$(git -C "$ROOT_DIR" rev-parse --short=12 HEAD 2>/dev/null || true)"
    INSTALLED_DIRTY="$(awk -F= '/^git_dirty=/ {print $2; exit}' "$BUILD_INFO_DEST")"

    if [ -n "$INSTALLED_HEAD" ] && [ -n "$CURRENT_HEAD" ] && [ "$INSTALLED_HEAD" = "$CURRENT_HEAD" ]; then
        pass "installed build git head matches the current checkout"
    elif [ -n "$INSTALLED_HEAD" ] && [ -n "$CURRENT_HEAD" ]; then
        fail "installed build git head is $INSTALLED_HEAD, current checkout is $CURRENT_HEAD"
    else
        warn "could not compare installed build git head with the current checkout"
    fi

    if [ "$INSTALLED_DIRTY" = "true" ]; then
        warn "installed build was produced from a dirty worktree"
    fi
else
    warn "build info is missing at $BUILD_INFO_DEST; run ./scripts/install-local-build.sh"
fi

if command -v pgrep >/dev/null 2>&1; then
    RUNNING_HOSTS="$(pgrep -a -u "$(id -u)" -f 'limux-host(.bin)?|libexec/limux/limux-host' 2>/dev/null || true)"
    if [ -n "$RUNNING_HOSTS" ]; then
        echo "Running Limux host processes:"
        printf '%s\n' "$RUNNING_HOSTS" | sed 's/^/  /'
    else
        echo "Running Limux host processes: none detected"
    fi
else
    warn "pgrep is not available; skipping running host process check"
fi

if [ "$failures" -ne 0 ]; then
    echo
    echo "Local dogfood status: FAILED"
    echo "Run ./scripts/install-local-build.sh, ensure $PREFIX/bin is first on PATH, then restart Limux."
    exit 1
fi

echo
echo "Local dogfood status: OK"
