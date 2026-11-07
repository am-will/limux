#!/usr/bin/env bash

LIMUX_WEBKIT_RUNTIME_DIR=""
LIMUX_WEBKIT_LIBRARY_DIR=""

cleanup_appimage_webkit_runtime() {
    [ -z "$LIMUX_WEBKIT_RUNTIME_DIR" ] || rm -rf -- "$LIMUX_WEBKIT_RUNTIME_DIR"
    [ -z "$LIMUX_WEBKIT_LIBRARY_DIR" ] || rm -rf -- "$LIMUX_WEBKIT_LIBRARY_DIR"
    LIMUX_WEBKIT_RUNTIME_DIR=""
    LIMUX_WEBKIT_LIBRARY_DIR=""
}

prepare_appimage_webkit_runtime() {
    local appdir="$1"
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/limux"
    local file relative target

    # WebKit uses this compiled path both for exec and absolute bubblewrap binds.
    # Keep the short helper path separate from executable files on noexec /tmp.
    LIMUX_WEBKIT_RUNTIME_DIR="$(mktemp -d /tmp/limux-wk-XXXXXX)" || return 1
    mkdir -p -- "$cache_dir" || return 1
    LIMUX_WEBKIT_LIBRARY_DIR="$(mktemp -d "$cache_dir/webkit-runtime.XXXXXX")" || return 1
    chmod 700 "$LIMUX_WEBKIT_RUNTIME_DIR" "$LIMUX_WEBKIT_LIBRARY_DIR" || return 1
    cp -a "$appdir/usr/lib/webkitgtk-6.0" "$LIMUX_WEBKIT_LIBRARY_DIR/webkitgtk-6.0" || return 1
    cp -as "$LIMUX_WEBKIT_LIBRARY_DIR/webkitgtk-6.0/." "$LIMUX_WEBKIT_RUNTIME_DIR/" || return 1
    chmod 700 "$LIMUX_WEBKIT_RUNTIME_DIR" || return 1
    while IFS= read -r -d '' file; do
        if ! grep -aFq '/proc/limux-wk-XXXXX' "$file"; then
            continue
        fi
        relative="${file#"$appdir/usr/lib/"}"
        target="$LIMUX_WEBKIT_LIBRARY_DIR/$relative"
        mkdir -p -- "$(dirname "$target")" || return 1
        LC_ALL=C sed "s|/proc/limux-wk-XXXXX|$LIMUX_WEBKIT_RUNTIME_DIR|g" "$file" > "$target" || return 1
        chmod --reference="$file" "$target" || return 1
    done < <(find "$appdir/usr/lib" -type f -name '*.so*' -print0)
    if [ ! -f "$LIMUX_WEBKIT_LIBRARY_DIR/libwebkitgtk-6.0.so.4" ]; then
        echo 'ERROR: AppImage WebKit runtime path was not patched.' >&2
        return 1
    fi
    if [ ! -x "$LIMUX_WEBKIT_LIBRARY_DIR/webkitgtk-6.0/WebKitWebProcess" ]; then
        echo 'ERROR: AppImage WebKit cache does not permit executable files.' >&2
        return 1
    fi
    export LD_LIBRARY_PATH="$LIMUX_WEBKIT_LIBRARY_DIR:$LD_LIBRARY_PATH"
    export WEBKIT_EXEC_PATH="$LIMUX_WEBKIT_RUNTIME_DIR"
    export WEBKIT_INJECTED_BUNDLE_PATH="$LIMUX_WEBKIT_RUNTIME_DIR/injected-bundle"
}
