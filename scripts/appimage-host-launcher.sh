#!/usr/bin/env bash
HERE="$(cd "$(dirname "$(readlink -f "$0")")/../../.." && pwd)"
unset LIMUX_HOST_BIN
unset LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE

# shellcheck source=scripts/appimage-webkit-runtime.sh
source "$HERE/usr/libexec/limux/appimage-webkit-runtime.sh"
LIMUX_CHILD_PID=""
# Activate the bundled gdk-pixbuf SVG loader. Its cache needs the current mount
# path, and the AppImage mount is read-only. Only export GDK_PIXBUF_MODULE_FILE
# when the cache is complete. A missing cache silently breaks SVG rendering.
prepare_pixbuf_loaders() {
    local pixbuf_dir="$HERE/usr/lib/gdk-pixbuf-2.0/2.10.0"
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/limux"
    local cache="" candidate contents loader

    [ -f "$pixbuf_dir/loaders.cache.template" ] && [ -d "$pixbuf_dir/loaders" ] || return 0
    contents="$(sed -e $'s\t@LOADER_DIR@\t'"$pixbuf_dir/loaders"$'\tg' \
        "$pixbuf_dir/loaders.cache.template")"
    [ -n "$contents" ] && mkdir -p -- "$cache_dir" 2>/dev/null || return 0

    # Remove the caches of mounts that are gone. Do not remove this one on exit,
    # because a window that outlives the launcher can still start another.
    for candidate in "$cache_dir"/pixbuf-loaders.cache.*; do
        [ -s "$candidate" ] || continue
        loader="$(sed -n '/^"/{s/^"//;s/"$//;p;q;}' "$candidate")"
        if [[ "$loader" == "$pixbuf_dir/loaders/"* ]]; then
            cache="$candidate"
        elif [ ! -e "$loader" ]; then
            rm -f -- "$candidate"
        fi
    done
    [ -n "$cache" ] \
        || cache="$(mktemp "$cache_dir/pixbuf-loaders.cache.XXXXXX" 2>/dev/null)" || return 0
    if printf '%s\n' "$contents" 2>/dev/null > "$cache"; then
        export GDK_PIXBUF_MODULE_FILE="$cache"
    fi
}
# shellcheck disable=SC2317
forward_signal() {
    if [ -n "$LIMUX_CHILD_PID" ]; then
        kill -s "$1" "$LIMUX_CHILD_PID" 2>/dev/null || true
    else
        cleanup_appimage_webkit_runtime
        trap - EXIT "$1"
        kill -s "$1" "$$"
    fi
}
trap cleanup_appimage_webkit_runtime EXIT
trap 'forward_signal TERM' TERM
trap 'forward_signal INT' INT
trap 'forward_signal HUP' HUP
prepare_pixbuf_loaders
if ! prepare_appimage_webkit_runtime "$HERE"; then
    export LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE=1
    echo 'WARNING: could not prepare the sandboxed AppImage WebKit runtime; browser views are unavailable.' >&2
    cleanup_appimage_webkit_runtime
fi

env --default-signal=INT,QUIT "$HERE/usr/libexec/limux/limux-host" "$@" <&0 &
LIMUX_CHILD_PID=$!
wait "$LIMUX_CHILD_PID"
LIMUX_EXIT_STATUS=$?
while kill -0 "$LIMUX_CHILD_PID" 2>/dev/null; do
    wait "$LIMUX_CHILD_PID"
    LIMUX_EXIT_STATUS=$?
done
exit "$LIMUX_EXIT_STATUS"
