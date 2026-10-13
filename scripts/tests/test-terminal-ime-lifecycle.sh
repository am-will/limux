#!/usr/bin/env bash
# Exercise GTK's IBus input-context lifecycle on a private X11 display.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"
for dependency in cargo dbus-monitor dbus-run-session ibus ibus-daemon setsid timeout xvfb-run; do
  command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency"; exit 2; }
done

if [ "${1:-}" != --inside ]; then
  "$ROOT_DIR/scripts/check-ghostty.sh"
  LIMUX_IME_TEST_DIR="$(mktemp -d -t limux-terminal-ime-XXXXXX)"
  export LIMUX_IME_TEST_DIR
  export XDG_DATA_HOME="$LIMUX_IME_TEST_DIR/data" XDG_STATE_HOME="$LIMUX_IME_TEST_DIR/state"
  export XDG_CONFIG_HOME="$LIMUX_IME_TEST_DIR/config" XDG_RUNTIME_DIR="$LIMUX_IME_TEST_DIR/runtime"
  export XDG_CACHE_HOME="$LIMUX_IME_TEST_DIR/cache"
  mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR" "$XDG_CACHE_HOME"
  chmod 700 "$XDG_RUNTIME_DIR"
  export GTK_USE_PORTAL=0 GTK_A11Y=none GIO_USE_VFS=local GSETTINGS_BACKEND=memory
  unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DISPLAY WAYLAND_DISPLAY
  unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK GPG_AGENT_INFO IBUS_ADDRESS IBUS_DAEMON_PID
  cat >"$LIMUX_IME_TEST_DIR/dbus.conf" <<'DBUS'
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
DBUS
  result=0
  xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
    dbus-run-session --config-file="$LIMUX_IME_TEST_DIR/dbus.conf" -- bash "$0" --inside \
    || result=$?
  if [ "$result" -eq 0 ]; then
    rm -rf "$LIMUX_IME_TEST_DIR"
  else
    echo "IME regression artifacts: $LIMUX_IME_TEST_DIR" >&2
  fi
  exit "$result"
fi

RUN_DIR="${LIMUX_IME_TEST_DIR:?}"
LOG_DIR="$RUN_DIR/logs"
mkdir -p "$LOG_DIR"
export GDK_BACKEND=x11 GDK_SCALE=1 GTK_IM_MODULE=ibus GTK_THEME=Adwaita
export GTK_USE_PORTAL=0 GTK_A11Y=none GIO_USE_VFS=local GSETTINGS_BACKEND=memory
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LP_NUM_THREADS=1
export LD_LIBRARY_PATH="$ROOT_DIR/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GHOSTTY_RESOURCES_DIR="$ROOT_DIR/ghostty/zig-out/share/ghostty"
export TERMINFO="$ROOT_DIR/ghostty/zig-out/share/terminfo"
export PKG_CONFIG="${PKG_CONFIG:-/usr/bin/pkg-config}"
unset LD_PRELOAD GDK_DPI_SCALE LIMUX_SOCKET LIMUX_SOCKET_PATH LIMUX_SOCKET_MODE
unset LIMUX_WORKSPACE_ID LIMUX_PANE_ID LIMUX_SURFACE_ID LIMUX_TAB_ID
export SHELL=/bin/sh ENV=/dev/null

IBUS_PID=""
MONITOR_PID=""
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  if [ -n "$MONITOR_PID" ]; then
    kill -TERM "$MONITOR_PID" 2>/dev/null || true
    wait "$MONITOR_PID" 2>/dev/null || true
  fi
  if [ -n "$IBUS_PID" ]; then
    kill -TERM -- "-$IBUS_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      if ! kill -0 -- "-$IBUS_PID" 2>/dev/null; then break; fi
      sleep 0.1
    done
    if kill -0 -- "-$IBUS_PID" 2>/dev/null; then
      kill -KILL -- "-$IBUS_PID" 2>/dev/null || true
    fi
    wait "$IBUS_PID" 2>/dev/null || true
  fi
  if [ "$result" -ne 0 ]; then
    for log in "$LOG_DIR"/*; do
      [ -f "$log" ] || continue
      echo "-- $log --" >&2
      tail -n 60 "$log" >&2 || true
    done
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Keep the daemon in a private process group so cleanup cannot signal the desktop IBus.
setsid ibus-daemon --panel=disable --config=disable --cache=none \
  >"$LOG_DIR/ibus-daemon.log" 2>&1 &
IBUS_PID=$!
IBUS_ADDRESS=""
for _ in $(seq 1 100); do
  IBUS_ADDRESS="$(ibus address 2>/dev/null || true)"
  if [[ "$IBUS_ADDRESS" == unix:* && "$IBUS_ADDRESS" != "(null)" ]]; then break; fi
  if ! kill -0 "$IBUS_PID" 2>/dev/null; then
    echo "IBus daemon exited during startup" >&2
    exit 1
  fi
  sleep 0.1
done
if [[ "$IBUS_ADDRESS" != unix:* || "$IBUS_ADDRESS" == "(null)" ]]; then
  echo "IBus daemon did not publish a private bus address" >&2
  exit 1
fi
export IBUS_ADDRESS

# Set an available engine to prove the daemon accepts client requests before GTK starts.
ibus engine xkb:us::eng >"$LOG_DIR/ibus-engine-set.log" 2>&1
if [ "$(ibus engine 2>"$LOG_DIR/ibus-engine-get.err")" != xkb:us::eng ]; then
  echo "Private IBus daemon did not select xkb:us::eng" >&2
  exit 1
fi
echo "IBus address: $IBUS_ADDRESS"

# Observe the private IBus bus itself. This makes a fallback or disconnected GTK
# input context fail the regression even if the test process exits successfully.
dbus-monitor --address "$IBUS_ADDRESS" "type='method_call'" \
  >"$LOG_DIR/ibus-monitor.log" 2>&1 &
MONITOR_PID=$!
sleep 0.1
kill -0 "$MONITOR_PID" || { echo "IBus monitor exited during startup"; exit 1; }

timeout -k 5s 900s cargo test --locked -p limux-host-linux \
  terminal::tests::ime_context_follows_terminal_realize_lifecycle \
  -- --exact --ignored --test-threads=1 --nocapture \
  >"$LOG_DIR/cargo-test.log" 2>&1 || { cat "$LOG_DIR/cargo-test.log"; exit 1; }
kill -0 "$IBUS_PID" 2>/dev/null || { echo "IBus daemon exited during regression"; exit 1; }
kill -0 "$MONITOR_PID" 2>/dev/null || { echo "IBus monitor exited during regression"; exit 1; }

kill -TERM "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true
MONITOR_PID=""
grep -F 'testing realized terminal with IBus input context' "$LOG_DIR/cargo-test.log" >/dev/null \
  || { echo "IME lifecycle regression did not run"; exit 1; }
grep -F 'member=CreateInputContext' "$LOG_DIR/ibus-monitor.log" >/dev/null \
  || { echo "GTK did not create a remote IBus input context"; exit 1; }

awk '
  /member=SetCursorLocation/ { reading = 1; count = 0; next }
  reading && $1 == "int32" {
    values[++count] = $2
    if (count == 4) {
      if (values[3] == 17 && values[4] == 19) first_reattach++
      if (values[3] == 18 && values[4] == 19) second_reattach++
      if (values[3] == 23 && values[4] == 29) shutdown++
      if (values[3] == 31 && values[4] == 37) unrealized++
      reading = 0
    }
  }
  END {
    if (!first_reattach || !second_reattach) {
      printf "Expected both reattached cursor updates, saw 17x19=%d and 18x19=%d\n", first_reattach, second_reattach > "/dev/stderr"
      exit 1
    }
    if (shutdown || unrealized) {
      printf "Detached context sent stale cursor updates: shutdown=%d unrealized=%d\n", shutdown, unrealized > "/dev/stderr"
      exit 1
    }
    printf "IBus trace: saw 17x19 and 18x19 reattach updates; no post-shutdown or unrealized updates\n"
  }
' "$LOG_DIR/ibus-monitor.log"

echo "PASS: GTK used the private IBus context through terminal detach, reattach, and shutdown"
