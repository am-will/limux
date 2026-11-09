#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="${LIMUX_TEST_CLI:-${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/limux-cli}"
[ -x "$CLI" ] || { echo "Build limux-cli before testing AppImage launch ownership: $CLI" >&2; exit 2; }
TEST_DIR="$(mktemp -d -t limux-appimage-launcher-XXXXXX)"
APP="$TEST_DIR/app"
APP_PID=""
cleanup() {
    if [ -n "$APP_PID" ]; then
        kill -KILL -- "-$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT
mkdir -p "$APP/usr/bin" "$APP/usr/libexec/limux" "$APP/usr/lib/webkitgtk-6.0"
cp "$CLI" "$APP/usr/bin/limux"
cp "$ROOT_DIR/scripts/appimage-host-launcher.sh" "$ROOT_DIR/scripts/appimage-webkit-runtime.sh" "$APP/usr/libexec/limux/"
awk '
    $0 == "cat > \"$APPDIR/AppRun\" << \047APPRUN_EOF\047" { copying = 1; next }
    copying && $0 == "APPRUN_EOF" { exit }
    copying { print }
' "$ROOT_DIR/scripts/package.sh" > "$APP/AppRun"
chmod 755 "$APP/AppRun"
printf '/proc/limux-wk-XXXXX\0' > "$APP/usr/lib/libwebkitgtk-6.0.so.4"
cp /bin/true "$APP/usr/lib/webkitgtk-6.0/WebKitWebProcess"
cat > "$APP/usr/libexec/limux/limux-host" <<'HOST'
#!/bin/sh
printf '%s' "${WEBKIT_EXEC_PATH:-}" > "$LIMUX_LAUNCHER_TEST_DIR/runtime"
printf '%s' "${LIMUX_HOST_BIN-unset}" > "$LIMUX_LAUNCHER_TEST_DIR/override"
if [ "$LIMUX_LAUNCHER_TEST_MODE" = once ]; then
    IFS= read -r input
    printf '%s' "$input"
    exit 37
fi
printf '%s' "$$" > "$LIMUX_LAUNCHER_TEST_DIR/host.pid"
trap '[ -d "$WEBKIT_EXEC_PATH" ] || exit 93; sleep 0.2; [ -d "$WEBKIT_EXEC_PATH" ] || exit 94; printf stopped > "$LIMUX_LAUNCHER_TEST_DIR/stopped"; exit 0' TERM
while :; do sleep 0.05; done
HOST
chmod 755 "$APP/usr/libexec/limux/limux-host"

export LIMUX_LAUNCHER_TEST_DIR="$TEST_DIR" XDG_CACHE_HOME="$TEST_DIR/cache"
unset LIMUX_HOST_BIN
"$APP/AppRun" --help > "$TEST_DIR/help"
grep -qF 'limux CLI' "$TEST_DIR/help"
[ ! -e "$XDG_CACHE_HOME/limux" ]

once() {
    local result
    set +e
    printf 'preserved launcher input\n' | LIMUX_LAUNCHER_TEST_MODE=once "$APP/AppRun" "$@" > "$TEST_DIR/output"
    result=$?
    set -e
    [ "$result" -eq 37 ]
    [ "$(cat "$TEST_DIR/output")" = 'preserved launcher input' ]
}
once
[ "$(cat "$TEST_DIR/override")" = unset ]
[ ! -e "$(cat "$TEST_DIR/runtime")" ]
once --socket-mode runtime --id-format refs
[ "$(cat "$TEST_DIR/override")" = unset ]
[ ! -e "$(cat "$TEST_DIR/runtime")" ]

cp "$APP/usr/libexec/limux/limux-host" "$TEST_DIR/custom-host"
export LIMUX_HOST_BIN="$TEST_DIR/custom-host"
once
[ "$(cat "$TEST_DIR/override")" = "$TEST_DIR/custom-host" ]
[ -z "$(cat "$TEST_DIR/runtime")" ]
unset LIMUX_HOST_BIN

LIMUX_LAUNCHER_TEST_MODE=signal setsid "$APP/AppRun" > "$TEST_DIR/output" 2> "$TEST_DIR/stderr" &
APP_PID=$!
for _ in $(seq 1 100); do
    [ ! -s "$TEST_DIR/host.pid" ] || break
    kill -0 "$APP_PID"
    sleep 0.05
done
[ -s "$TEST_DIR/host.pid" ]
runtime="$(cat "$TEST_DIR/runtime")"
[ -d "$runtime" ]
kill -TERM "$APP_PID"
for _ in $(seq 1 100); do
    if ! kill -0 "$APP_PID" 2>/dev/null; then break; fi
    sleep 0.05
done
if kill -0 "$APP_PID" 2>/dev/null; then
    echo 'FAIL: AppImage launcher did not finish after its host handled TERM.' >&2
    exit 1
fi
wait "$APP_PID"
APP_PID=""
[ -f "$TEST_DIR/stopped" ]
[ ! -e "$runtime" ]
if kill -0 "$(cat "$TEST_DIR/host.pid")" 2>/dev/null; then
    echo 'FAIL: the GUI host survived AppImage launcher cleanup.' >&2
    exit 1
fi
echo 'AppImage launch routing, stdin, signal forwarding, and runtime lifetime: OK'
