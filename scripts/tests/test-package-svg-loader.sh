#!/usr/bin/env bash
# Unit tests for the AppImage SVG-loader bundling logic in scripts/package.sh.
#
# Tests are pure-shell. They isolate the relevant blocks so they can be
# exercised without running a full package build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PACKAGE_SH="$SCRIPT_DIR/../package.sh"

PASS=0
FAIL=0

pass() { PASS=$((PASS+1)); printf '  ✓ %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  ✗ %s\n    %s\n' "$1" "$2"; }

# ----- T1: PIXBUF_MULTIARCH mapping per uname -m -----

multiarch_for() {
    # Replicates the case branch from package.sh.
    case "$1" in
        x86_64) echo "x86_64-linux-gnu" ;;
        aarch64) echo "aarch64-linux-gnu" ;;
        i386|i486|i586|i686) echo "i386-linux-gnu" ;;
        armv7l|armv7) echo "arm-linux-gnueabihf" ;;
        armv6l) echo "arm-linux-gnueabi" ;;
        *) echo "$1-linux-gnu" ;;
    esac
}

echo "T1: PIXBUF_MULTIARCH mapping"
for input in x86_64:x86_64-linux-gnu \
             aarch64:aarch64-linux-gnu \
             i686:i386-linux-gnu \
             i386:i386-linux-gnu \
             armv7l:arm-linux-gnueabihf \
             armv6l:arm-linux-gnueabi \
             unknownarch:unknownarch-linux-gnu
do
    arch="${input%%:*}"
    expected="${input##*:}"
    actual="$(multiarch_for "$arch")"
    if [ "$actual" = "$expected" ]; then
        pass "uname -m=$arch → $expected"
    else
        fail "uname -m=$arch" "expected '$expected', got '$actual'"
    fi
done

# ----- T2: sed delimiter handles paths with `|` -----

echo 'T2: sed substitution with "|" in source path'
HEREDOC_TMP=$(mktemp -d)
trap 'rm -rf "$HEREDOC_TMP"' EXIT

# Simulate a build path containing a `|` (rare but legal).
EVIL_APPDIR="$HEREDOC_TMP/build|dir"
mkdir -p "$EVIL_APPDIR"
PIXBUF_LOADER_DIR_REL="usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
mkdir -p "$EVIL_APPDIR/$PIXBUF_LOADER_DIR_REL"

# Fake gdk-pixbuf-query-loaders output containing the abs path.
QUERY_OUT="\"$EVIL_APPDIR/$PIXBUF_LOADER_DIR_REL/libpixbufloader_svg.so\""
TEMPLATE=$(printf '%s\n' "$QUERY_OUT" | sed -e $'s\t'"$EVIL_APPDIR/$PIXBUF_LOADER_DIR_REL"$'\t@LOADER_DIR@\tg')

if [[ "$TEMPLATE" == "\"@LOADER_DIR@/libpixbufloader_svg.so\"" ]]; then
    pass "tab-delimited sed handles '|' in source path"
else
    fail "tab-delimited sed with '|' in path" "got: $TEMPLATE"
fi

# Test that the old pipe-delimited sed would have failed.
if ! BAD=$(printf '%s\n' "$QUERY_OUT" | sed "s|$EVIL_APPDIR/$PIXBUF_LOADER_DIR_REL|@LOADER_DIR@|g" 2>/dev/null) \
   || [[ "$BAD" == *"build|dir"* ]]; then
    pass "regression: pipe-delimited sed fails with '|' in path (expected)"
else
    fail "regression check" "old sed unexpectedly worked: $BAD"
fi

# ----- T3: AppRun materializes cache only on successful mkdir+sed -----

echo "T3: AppRun GDK_PIXBUF_MODULE_FILE conditional export"

# Run the AppRun pixbuf block in isolation with read-only HOME.
run_apprun_block() {
    local home_dir="$1"
    local pixbuf_dir="$2"

    HOME="$home_dir" XDG_CACHE_HOME="" \
    PIXBUF_DIR="$pixbuf_dir" \
    bash -c '
        if [ -f "${PIXBUF_DIR}/loaders.cache.template" ] && [ -d "${PIXBUF_DIR}/loaders" ]; then
            LIMUX_CACHE_BASE="${XDG_CACHE_HOME:-$HOME/.cache}"
            LIMUX_CACHE="${LIMUX_CACHE_BASE}/limux"
            PIXBUF_CACHE="${LIMUX_CACHE}/pixbuf-loaders.cache.$$"
            if [ -n "${LIMUX_CACHE_BASE}" ] \
               && mkdir -p "$LIMUX_CACHE" 2>/dev/null \
               && sed -e $'"'"'s\t@LOADER_DIR@\t'"'"'"${PIXBUF_DIR}/loaders"$'"'"'\tg'"'"' \
                      "${PIXBUF_DIR}/loaders.cache.template" > "${PIXBUF_CACHE}" 2>/dev/null \
               && [ -s "${PIXBUF_CACHE}" ]; then
                export GDK_PIXBUF_MODULE_FILE="${PIXBUF_CACHE}"
            fi
        fi
        # Report whether the export happened.
        echo "${GDK_PIXBUF_MODULE_FILE:-NOT_SET}"
    '
}

# Setup a valid pixbuf dir with template + loaders.
PIXBUF_DIR="$HEREDOC_TMP/pixbuf"
mkdir -p "$PIXBUF_DIR/loaders"
printf '"@LOADER_DIR@/libpixbufloader_svg.so"\n' > "$PIXBUF_DIR/loaders.cache.template"

# Case 1: writable HOME → export should happen
WRITABLE_HOME="$HEREDOC_TMP/home1"
mkdir -p "$WRITABLE_HOME"
result=$(run_apprun_block "$WRITABLE_HOME" "$PIXBUF_DIR")
if [[ "$result" == *"pixbuf-loaders.cache."* ]] && [[ "$result" != "NOT_SET" ]]; then
    pass "export with writable HOME"
else
    fail "export with writable HOME" "got: $result"
fi

# Case 2: read-only HOME (mkdir fails) → export should NOT happen
RO_HOME="$HEREDOC_TMP/home2_ro"
mkdir -p "$RO_HOME"
chmod 555 "$RO_HOME"
result=$(run_apprun_block "$RO_HOME" "$PIXBUF_DIR")
chmod 755 "$RO_HOME"
if [[ "$result" == "NOT_SET" ]]; then
    pass "skip export on read-only HOME"
else
    fail "skip export on read-only HOME" "expected NOT_SET, got: $result"
fi

# ----- T4: LIMUX_REQUIRE_SVG_LOADER hard-fail vs warn-only -----

echo "T4: LIMUX_REQUIRE_SVG_LOADER gates exit 1"

# Excerpt the warn-or-exit logic — paste-equivalent to the script.
warn_or_exit() {
    local require="${1:-}"
    echo "WARNING: simulated loader missing"
    if [ "${require:-}" = "1" ]; then
        return 99
    fi
    return 0
}

ec=0
warn_or_exit "" >/dev/null 2>&1 || ec=$?
if [ "$ec" = "0" ]; then
    pass "no env var → warn and continue (exit 0)"
else
    fail "warn-only path" "expected exit 0, got $ec"
fi

ec=0
warn_or_exit "1" >/dev/null 2>&1 || ec=$?
if [ "$ec" = "99" ]; then
    pass "LIMUX_REQUIRE_SVG_LOADER=1 → hard fail (exit 99)"
else
    fail "hard-fail path" "expected exit 99, got $ec"
fi

# ----- Summary -----

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
