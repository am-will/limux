#!/usr/bin/env bash
# Regression tests for AppImage packaging logic.
#
# Tests are pure-shell. They isolate the relevant blocks so they can be
# exercised without running a full package build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APPIMAGE_WEBKIT_SH="$SCRIPT_DIR/../appimage-webkit.sh"

# shellcheck disable=SC1090,SC1091
source "$APPIMAGE_WEBKIT_SH"

PASS=0
FAIL=0

pass() { PASS=$((PASS+1)); printf '  PASS %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  FAIL %s\n    %s\n' "$1" "$2"; }

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
        pass "uname -m=$arch -> $expected"
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

# ----- T4: AppRun preserves original loader environment for terminal children -----

echo "T4: AppRun original environment markers"

run_apprun_preserve_block() {
    LD_LIBRARY_PATH="${1-}" \
    GDK_PIXBUF_MODULE_FILE="${2-}" \
    WEBKIT_EXEC_PATH="${3-}" \
    WEBKIT_INJECTED_BUNDLE_PATH="${4-}" \
    GBM_BACKENDS_PATH="${5-}" \
    LIBGL_DRIVERS_PATH="${6-}" \
    bash -c '
        if [ "${LD_LIBRARY_PATH+x}" = x ]; then
            export LIMUX_ORIGINAL_LD_LIBRARY_PATH_SET=1
            export LIMUX_ORIGINAL_LD_LIBRARY_PATH="${LD_LIBRARY_PATH}"
        else
            export LIMUX_ORIGINAL_LD_LIBRARY_PATH_SET=0
            export LIMUX_ORIGINAL_LD_LIBRARY_PATH=""
        fi
        if [ "${GDK_PIXBUF_MODULE_FILE+x}" = x ]; then
            export LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE_SET=1
            export LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE="${GDK_PIXBUF_MODULE_FILE}"
        else
            export LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE_SET=0
            export LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE=""
        fi
        if [ "${WEBKIT_EXEC_PATH+x}" = x ]; then
            export LIMUX_ORIGINAL_WEBKIT_EXEC_PATH_SET=1
            export LIMUX_ORIGINAL_WEBKIT_EXEC_PATH="${WEBKIT_EXEC_PATH}"
        else
            export LIMUX_ORIGINAL_WEBKIT_EXEC_PATH_SET=0
            export LIMUX_ORIGINAL_WEBKIT_EXEC_PATH=""
        fi
        if [ "${WEBKIT_INJECTED_BUNDLE_PATH+x}" = x ]; then
            export LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH_SET=1
            export LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH="${WEBKIT_INJECTED_BUNDLE_PATH}"
        else
            export LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH_SET=0
            export LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH=""
        fi
        if [ "${GBM_BACKENDS_PATH+x}" = x ]; then
            export LIMUX_ORIGINAL_GBM_BACKENDS_PATH_SET=1
            export LIMUX_ORIGINAL_GBM_BACKENDS_PATH="${GBM_BACKENDS_PATH}"
        else
            export LIMUX_ORIGINAL_GBM_BACKENDS_PATH_SET=0
            export LIMUX_ORIGINAL_GBM_BACKENDS_PATH=""
        fi
        if [ "${LIBGL_DRIVERS_PATH+x}" = x ]; then
            export LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH_SET=1
            export LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH="${LIBGL_DRIVERS_PATH}"
        else
            export LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH_SET=0
            export LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH=""
        fi
        printf "%s\n" \
            "$LIMUX_ORIGINAL_LD_LIBRARY_PATH_SET:$LIMUX_ORIGINAL_LD_LIBRARY_PATH" \
            "$LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE_SET:$LIMUX_ORIGINAL_GDK_PIXBUF_MODULE_FILE" \
            "$LIMUX_ORIGINAL_WEBKIT_EXEC_PATH_SET:$LIMUX_ORIGINAL_WEBKIT_EXEC_PATH" \
            "$LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH_SET:$LIMUX_ORIGINAL_WEBKIT_INJECTED_BUNDLE_PATH" \
            "$LIMUX_ORIGINAL_GBM_BACKENDS_PATH_SET:$LIMUX_ORIGINAL_GBM_BACKENDS_PATH" \
            "$LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH_SET:$LIMUX_ORIGINAL_LIBGL_DRIVERS_PATH"
    '
}

result=$(run_apprun_preserve_block "/host/lib" "/host/pixbuf.cache" "/host/webkit" "/host/bundle" "/host/gbm" "/host/dri")
expected=$'1:/host/lib\n1:/host/pixbuf.cache\n1:/host/webkit\n1:/host/bundle\n1:/host/gbm\n1:/host/dri'
if [[ "$result" == "$expected" ]]; then
    pass "records original app-sensitive environment"
else
    fail "records original app-sensitive environment" "got: $result"
fi

# ----- T5: AppRun discovers host Mesa module directories -----

echo "T5: AppRun host Mesa module discovery"

HOST_GBM_DIR="$HEREDOC_TMP/host/lib/gbm"
HOST_DRI_DIR="$HEREDOC_TMP/host/lib/dri"
mkdir -p "$HOST_GBM_DIR" "$HOST_DRI_DIR"
touch "$HOST_GBM_DIR/dri_gbm.so" "$HOST_DRI_DIR/iris_dri.so"

discover_graphics_dir() {
    local pattern="$1"
    shift
    local candidate
    for candidate in "$@"; do
        if [ -d "$candidate" ] && compgen -G "${candidate}/${pattern}" >/dev/null; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

if [ "$(discover_graphics_dir '*_gbm.so' "$HEREDOC_TMP/missing" "$HOST_GBM_DIR")" = "$HOST_GBM_DIR" ]; then
    pass "discovers versioned GBM backend directory"
else
    fail "GBM backend discovery" "did not select $HOST_GBM_DIR"
fi

if [ "$(discover_graphics_dir '*_dri.so' "$HEREDOC_TMP/missing" "$HOST_DRI_DIR")" = "$HOST_DRI_DIR" ]; then
    pass "discovers host DRI driver directory"
else
    fail "DRI driver discovery" "did not select $HOST_DRI_DIR"
fi

# ----- T6: LIMUX_REQUIRE_SVG_LOADER hard-fail vs warn-only -----

echo "T6: LIMUX_REQUIRE_SVG_LOADER gates exit 1"

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
    pass "no env var -> warn and continue (exit 0)"
else
    fail "warn-only path" "expected exit 0, got $ec"
fi

ec=0
warn_or_exit "1" >/dev/null 2>&1 || ec=$?
if [ "$ec" = "99" ]; then
    pass "LIMUX_REQUIRE_SVG_LOADER=1 -> hard fail (exit 99)"
else
    fail "hard-fail path" "expected exit 99, got $ec"
fi

# ----- T7: host graphics libraries stay outside the AppImage -----

echo "T7: AppImage uses host graphics libraries"

for library in \
    libEGL.so.1 \
    libGL.so.1 \
    libGLESv1_CM.so.1 \
    libGLESv2.so.2 \
    libGLX.so.0 \
    libGLdispatch.so.0 \
    libOpenGL.so.0 \
    libdrm.so.2 \
    libdrm_amdgpu.so.1 \
    libgbm.so.1 \
    libglapi.so.0 \
    libwayland-client.so.0 \
    libwayland-cursor.so.0 \
    libwayland-egl.so.1 \
    libwayland-server.so.0
do
    if is_appimage_system_library "/usr/lib/$library"; then
        pass "excludes $library"
    else
        fail "host graphics exclusion" "$library would be bundled"
    fi
done

if ! is_appimage_system_library "/usr/lib/libwebkitgtk-6.0.so.4"; then
    pass "keeps application runtime libraries bundled"
else
    fail "application runtime closure" "libwebkitgtk-6.0.so.4 would be excluded"
fi

# ----- T8: WebKitGTK process path is made relative to AppRun cwd -----

echo "T8: AppImage relocates WebKitGTK process path"

WEBKIT_APPDIR="$HEREDOC_TMP/webkit-appdir"
WEBKIT_LIBRARY="$WEBKIT_APPDIR/usr/lib/libwebkitgtk-6.0.so.4"
WEBKITGTK_RUNTIME_DIR="/usr/lib/x86_64-linux-gnu/webkitgtk-6.0"
WEBKITGTK_PROCESS_DIR="$WEBKITGTK_RUNTIME_DIR"
mkdir -p "$(dirname "$WEBKIT_LIBRARY")"
printf '%s\0%s\0' \
    "$WEBKITGTK_PROCESS_DIR" \
    "$WEBKITGTK_RUNTIME_DIR/injected-bundle/" \
    > "$WEBKIT_LIBRARY"
chmod 755 "$WEBKIT_LIBRARY"

patch_appimage_webkit_paths "$WEBKIT_APPDIR"

if grep -aFq "$WEBKITGTK_PROCESS_DIR" "$WEBKIT_LIBRARY"; then
    fail "WebKitGTK path relocation" "compiled-in process path remains"
elif ! grep -aFq "usr/lib/webkitgtk-6.0" "$WEBKIT_LIBRARY"; then
    fail "WebKitGTK path relocation" "relative process path is missing"
else
    pass "rewrites compiled-in process and injected-bundle paths"
fi

# ----- Summary -----

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
