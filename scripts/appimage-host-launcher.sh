#!/usr/bin/env bash
HERE="$(cd "$(dirname "$(readlink -f "$0")")/../../.." && pwd)"
unset LIMUX_HOST_BIN
unset LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE

# shellcheck source=scripts/appimage-webkit-runtime.sh
source "$HERE/usr/libexec/limux/appimage-webkit-runtime.sh"
trap cleanup_appimage_webkit_runtime EXIT
if ! prepare_appimage_webkit_runtime "$HERE"; then
    export LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE=1
    echo 'WARNING: could not prepare the sandboxed AppImage WebKit runtime; browser views are unavailable.' >&2
    cleanup_appimage_webkit_runtime
fi

LIMUX_CHILD_PID=""
# shellcheck disable=SC2317
forward_signal() {
    [ -z "$LIMUX_CHILD_PID" ] || kill -s "$1" "$LIMUX_CHILD_PID" 2>/dev/null || true
}
trap 'forward_signal TERM' TERM
trap 'forward_signal INT' INT
trap 'forward_signal HUP' HUP
env --default-signal=INT,QUIT "$HERE/usr/libexec/limux/limux-host" "$@" <&0 &
LIMUX_CHILD_PID=$!
wait "$LIMUX_CHILD_PID"
LIMUX_EXIT_STATUS=$?
while kill -0 "$LIMUX_CHILD_PID" 2>/dev/null; do
    wait "$LIMUX_CHILD_PID"
    LIMUX_EXIT_STATUS=$?
done
exit "$LIMUX_EXIT_STATUS"
