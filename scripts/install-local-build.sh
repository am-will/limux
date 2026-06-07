#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${LIMUX_LOCAL_PREFIX:-$HOME/.local}"
PROFILE="${LIMUX_LOCAL_PROFILE:-release}"
RUST_TOOLCHAIN="${LIMUX_RUST_TOOLCHAIN:-}"

case "$PROFILE" in
    debug)
        CARGO_FLAGS=()
        TARGET_DIR="$ROOT_DIR/target/debug"
        ;;
    release)
        CARGO_FLAGS=(--release)
        TARGET_DIR="$ROOT_DIR/target/release"
        ;;
    *)
        echo "ERROR: LIMUX_LOCAL_PROFILE must be debug or release, got '$PROFILE'." >&2
        exit 1
        ;;
esac

CLI_SRC="$TARGET_DIR/limux-cli"
HOST_SRC="$TARGET_DIR/limux"
CLI_DEST="$PREFIX/bin/limux"
HOST_DIR="$PREFIX/libexec/limux"
HOST_WRAPPER="$HOST_DIR/limux-host"
HOST_BIN_DEST="$HOST_DIR/limux-host.bin"
LIB_DEST="$PREFIX/lib/limux/libghostty.so"
DESKTOP_SRC="$ROOT_DIR/rust/limux-host-linux/dev.limux.linux.desktop"
DESKTOP_DEST="$PREFIX/share/applications/dev.limux.linux.desktop"
BUILD_INFO_DEST="$PREFIX/share/limux/local-build.txt"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

sha256_file() {
    sha256sum "$1" | awk '{print $1}'
}

resolve_first_file() {
    local candidate

    for candidate in "$@"; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

resolve_first_dir() {
    local candidate

    for candidate in "$@"; do
        if [ -d "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

copy_dir_contents() {
    local src="$1"
    local dest="$2"

    mkdir -p "$dest"
    cp -R "$src"/. "$dest"/
}

install_file_atomic() {
    local src="$1"
    local dest="$2"
    local mode="$3"
    local dir
    local tmp

    dir="$(dirname "$dest")"
    mkdir -p "$dir"
    tmp="$(mktemp "$dir/.install.XXXXXX")"
    cp "$src" "$tmp"
    chmod "$mode" "$tmp"
    mv -f "$tmp" "$dest"
}

install_desktop_file() {
    mkdir -p "$(dirname "$DESKTOP_DEST")"
    sed \
        -e "s|^Exec=.*|Exec=${CLI_DEST}|" \
        -e "s|^TryExec=.*|TryExec=${CLI_DEST}|" \
        "$DESKTOP_SRC" > "$DESKTOP_DEST"
    chmod 644 "$DESKTOP_DEST"
}

write_host_wrapper() {
    mkdir -p "$HOST_DIR"
    local tmp
    tmp="$(mktemp "$HOST_DIR/.limux-host.XXXXXX")"
    cat > "$tmp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PREFIX="$(cd "$HERE/../.." && pwd)"
export LD_LIBRARY_PATH="$PREFIX/lib/limux${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/limux-host.bin" "$@"
EOF
    chmod 755 "$tmp"
    mv -f "$tmp" "$HOST_WRAPPER"
}

need_cmd awk
need_cmd cargo
need_cmd cp
need_cmd ldd
need_cmd mktemp
need_cmd sha256sum

cd "$ROOT_DIR"

CARGO_CMD=(cargo)
if [ -n "$RUST_TOOLCHAIN" ]; then
    CARGO_CMD=(cargo "+$RUST_TOOLCHAIN")
elif cargo +1.92 --version >/dev/null 2>&1; then
    CARGO_CMD=(cargo +1.92)
fi

echo "Building Limux local $PROFILE binaries..."
"${CARGO_CMD[@]}" build "${CARGO_FLAGS[@]}" -p limux-cli --bin limux-cli
"${CARGO_CMD[@]}" build "${CARGO_FLAGS[@]}" -p limux-host-linux --bin limux

[ -x "$CLI_SRC" ] || fail "CLI binary not found at $CLI_SRC"
[ -x "$HOST_SRC" ] || fail "host binary not found at $HOST_SRC"

GHOSTTY_LIB_SRC="$(resolve_first_file \
    "$ROOT_DIR/ghostty/zig-out/lib/libghostty.so" \
    /usr/local/lib/limux/libghostty.so \
    /usr/lib/limux/libghostty.so)" \
    || fail "libghostty.so not found; build Ghostty or install Limux first"

install_file_atomic "$CLI_SRC" "$CLI_DEST" 755
install_file_atomic "$HOST_SRC" "$HOST_BIN_DEST" 755
write_host_wrapper
install_file_atomic "$GHOSTTY_LIB_SRC" "$LIB_DEST" 644

if GHOSTTY_RESOURCES_SRC="$(resolve_first_dir \
    "$ROOT_DIR/ghostty/zig-out/share/ghostty" \
    /usr/local/share/limux/ghostty \
    /usr/share/limux/ghostty \
    /usr/local/share/ghostty \
    /usr/share/ghostty)"; then
    copy_dir_contents "$GHOSTTY_RESOURCES_SRC" "$PREFIX/share/limux/ghostty"
else
    echo "WARNING: Ghostty resources were not found; existing runtime fallbacks will be used." >&2
fi

if GHOSTTY_TERMINFO_SRC="$(resolve_first_dir \
    "$ROOT_DIR/ghostty/zig-out/share/terminfo" \
    /usr/local/share/limux/terminfo \
    /usr/share/limux/terminfo \
    /usr/local/share/terminfo \
    /usr/share/terminfo)"; then
    copy_dir_contents "$GHOSTTY_TERMINFO_SRC" "$PREFIX/share/limux/terminfo"
else
    echo "WARNING: Ghostty terminfo was not found; existing runtime fallbacks will be used." >&2
fi

if [ -f "$DESKTOP_SRC" ]; then
    install_desktop_file
fi

mkdir -p "$(dirname "$BUILD_INFO_DEST")"
{
    printf 'profile=%s\n' "$PROFILE"
    printf 'installed_at=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    printf 'git_head=%s\n' "$(git rev-parse --short=12 HEAD 2>/dev/null || printf unknown)"
    printf 'git_dirty=%s\n' "$(test -z "$(git status --short 2>/dev/null)" && printf false || printf true)"
    printf 'cli_sha256=%s\n' "$(sha256_file "$CLI_DEST")"
    printf 'host_sha256=%s\n' "$(sha256_file "$HOST_BIN_DEST")"
    printf 'libghostty_sha256=%s\n' "$(sha256_file "$LIB_DEST")"
} > "$BUILD_INFO_DEST"

ACTIVE_LIMUX="$(command -v limux 2>/dev/null || true)"
[ -n "$ACTIVE_LIMUX" ] || fail "limux is not on PATH after installing $CLI_DEST"

if [ "$(readlink -f "$ACTIVE_LIMUX")" != "$(readlink -f "$CLI_DEST")" ]; then
    fail "active limux is $ACTIVE_LIMUX, expected $CLI_DEST. Put $PREFIX/bin before older Limux installs on PATH."
fi

"$CLI_DEST" --help 2>&1 | grep -q "limux CLI" \
    || fail "$CLI_DEST is not the Limux CLI entrypoint"

[ "$(sha256_file "$CLI_SRC")" = "$(sha256_file "$CLI_DEST")" ] \
    || fail "installed CLI hash does not match $CLI_SRC"
[ "$(sha256_file "$HOST_SRC")" = "$(sha256_file "$HOST_BIN_DEST")" ] \
    || fail "installed host hash does not match $HOST_SRC"

RESOLVED_LIB="$(
    LD_LIBRARY_PATH="$PREFIX/lib/limux${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        ldd "$HOST_BIN_DEST" \
        | awk '/libghostty\.so/ {print $3; exit}'
)"
[ -n "$RESOLVED_LIB" ] || fail "host does not resolve libghostty.so"

if [ "$(readlink -f "$RESOLVED_LIB")" != "$(readlink -f "$LIB_DEST")" ]; then
    fail "host resolves libghostty.so to $RESOLVED_LIB, expected $LIB_DEST"
fi

"$HOST_WRAPPER" --version >/dev/null \
    || fail "installed host wrapper failed to execute"

echo "Installed active local Limux build."
echo "  CLI:       $CLI_DEST"
echo "  Host:      $HOST_WRAPPER -> $HOST_BIN_DEST"
echo "  Library:   $LIB_DEST"
echo "  Build info: $BUILD_INFO_DEST"
echo "Restart any already-running Limux GUI to use this host build."
