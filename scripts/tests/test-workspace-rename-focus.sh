#!/usr/bin/env bash
# Exercise terminal focus after an inline workspace rename is interrupted by
# another X11 application, then test that an intentional click still wins.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
"$ROOT_DIR/scripts/check-ghostty.sh"
for dependency in xvfb-run xdotool metacity xmessage dbus-run-session jq setsid; do
  command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency"; exit 2; }
done
if [ "${1:-}" != --inside ]; then
  LIMUX_RENAME_FOCUS_TEST_DIR="$(mktemp -d -t limux-rename-focus-XXXXXX)"
  export LIMUX_RENAME_FOCUS_TEST_DIR
  export XDG_DATA_HOME="$LIMUX_RENAME_FOCUS_TEST_DIR/data" XDG_STATE_HOME="$LIMUX_RENAME_FOCUS_TEST_DIR/state"
  export XDG_CONFIG_HOME="$LIMUX_RENAME_FOCUS_TEST_DIR/config" XDG_RUNTIME_DIR="$LIMUX_RENAME_FOCUS_TEST_DIR/runtime"
  export XDG_CACHE_HOME="$LIMUX_RENAME_FOCUS_TEST_DIR/cache"
  mkdir -p "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME/ghostty" "$XDG_RUNTIME_DIR" "$XDG_CACHE_HOME"
  chmod 700 "$XDG_RUNTIME_DIR"
  export GTK_USE_PORTAL=0 GTK_A11Y=none GIO_USE_VFS=local GSETTINGS_BACKEND=memory
  unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DISPLAY WAYLAND_DISPLAY
  unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK GPG_AGENT_INFO
  cat >"$LIMUX_RENAME_FOCUS_TEST_DIR/dbus.conf" <<'DBUS'
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
  exec xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
    dbus-run-session --config-file="$LIMUX_RENAME_FOCUS_TEST_DIR/dbus.conf" -- bash "$0" --inside
fi

HOST="${LIMUX_TEST_HOST:-$ROOT_DIR/target/debug/limux}"
CLI="${LIMUX_TEST_CLI:-$ROOT_DIR/target/debug/limux-cli}"
if [ ! -x "$HOST" ] || [ ! -x "$CLI" ]; then echo "Build the host and CLI first"; exit 2; fi
RUN_DIR="${LIMUX_RENAME_FOCUS_TEST_DIR:?}"
SOCKET="$RUN_DIR/limux.sock"
echo "Test artifacts: $RUN_DIR"
export LIMUX_SOCKET="$SOCKET" LIMUX_SOCKET_PATH="$SOCKET" LIMUX_SOCKET_MODE=localUser
export GDK_BACKEND=x11 GDK_SCALE=1 GTK_THEME=Adwaita:dark
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LP_NUM_THREADS=1 SHELL=/bin/sh ENV=/dev/null
export LD_LIBRARY_PATH="$ROOT_DIR/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GHOSTTY_RESOURCES_DIR="$ROOT_DIR/ghostty/zig-out/share/ghostty"
export TERMINFO="$ROOT_DIR/ghostty/zig-out/share/terminfo"
unset LIMUX_WORKSPACE_ID LIMUX_PANE_ID LIMUX_SURFACE_ID LIMUX_TAB_ID LD_PRELOAD GDK_DPI_SCALE
unset XDG_ACTIVATION_TOKEN DESKTOP_STARTUP_ID
cli() { "$CLI" --socket "$SOCKET" "$@"; }
printf 'command = /bin/sh\nshell-integration = none\nfont-size = 12\n' >"$XDG_CONFIG_HOME/ghostty/config"
jq -n --arg cwd "$RUN_DIR" '{version: 1, active_workspace_index: 0,
  top_bar_visible: true, sidebar: {visible: true, width: 220},
  workspaces: [{id: "other", name: "Other", cwd: $cwd,
    layout: {kind: "pane", pane_id: 1, active_tab_id: "other", tabs: [
      {id: "other", tab_kind: "terminal", cwd: $cwd, custom_name: "Other"}
    ]}}]
}' >"$XDG_DATA_HOME/limux/session.json"

HOST_PID=""
WM_PID=""
XMESSAGE_PID=""
# shellcheck disable=SC2317 # Invoked by the EXIT trap.
cleanup() {
  local result=$? pid
  if [ "$result" -ne 0 ]; then
    if command -v import >/dev/null; then import -window root "$RUN_DIR/failure.png" || true; fi
    if [ -n "${WORKSPACE_ID:-}" ]; then cli --json list-workspaces >"$RUN_DIR/failure-workspaces.json" 2>/dev/null || true; fi
    tail -n 40 "$RUN_DIR/host.stderr" "$RUN_DIR/wm.stderr" 2>/dev/null || true
  fi
  if [ -n "$XMESSAGE_PID" ]; then kill "$XMESSAGE_PID" 2>/dev/null || true; wait "$XMESSAGE_PID" 2>/dev/null || true; fi
  for pid in "$HOST_PID" "$WM_PID"; do
    [ -n "$pid" ] || continue
    kill -TERM -- "-$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
      if ! kill -0 "$pid" 2>/dev/null; then break; fi
      sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then kill -KILL -- "-$pid" 2>/dev/null || true; fi
    wait "$pid" 2>/dev/null || true
  done
  if [ -n "${LIMUX_TEST_ARTIFACT_DIR:-}" ]; then
    mkdir -p "$LIMUX_TEST_ARTIFACT_DIR"
    for artifact in "$RUN_DIR"/*.png "$RUN_DIR"/*.stderr; do
      [ -f "$artifact" ] || continue
      cp "$artifact" "$LIMUX_TEST_ARTIFACT_DIR/" || true
    done
  fi
}
trap cleanup EXIT

setsid metacity --replace --sm-disable --no-composite >"$RUN_DIR/wm.stdout" 2>"$RUN_DIR/wm.stderr" &
WM_PID=$!
for _ in $(seq 1 50); do
  kill -0 "$WM_PID" || { echo "Window manager exited during startup"; exit 1; }
  if xdotool get_desktop >/dev/null 2>&1; then break; fi
  sleep 0.1
done
xdotool get_desktop >/dev/null
setsid "$HOST" >"$RUN_DIR/host.stdout" 2>"$RUN_DIR/host.stderr" &
HOST_PID=$!
WINDOW=""
for _ in $(seq 1 450); do
  kill -0 "$HOST_PID" || { echo "Host exited during startup"; exit 1; }
  WINDOW="$(xdotool search --all --onlyvisible --pid "$HOST_PID" --name '^Limux v' 2>/dev/null | head -1 || true)"
  if [ -n "$WINDOW" ] && cli --json identify >"$RUN_DIR/ready.json" 2>/dev/null \
    && jq -e '.focused.name == "Other" and .focused.surface_ref == "surface:1:other"' \
      "$RUN_DIR/ready.json" >/dev/null; then break; fi
  sleep 0.1
done
if [ -z "$WINDOW" ]; then echo "Host startup timed out"; exit 1; fi
xdotool windowsize --sync "$WINDOW" 1200 760 windowactivate --sync "$WINDOW"
sleep 0.5

# Create a second workspace through the production control path, explicitly
# select it, and add another terminal tab for the deliberate-click case.
cli --json new-workspace --cwd "$RUN_DIR" >"$RUN_DIR/created-workspace.json"
WORKSPACE_ID="$(jq -r '.workspace_ref' "$RUN_DIR/created-workspace.json")"
if [ -z "$WORKSPACE_ID" ] || [ "$WORKSPACE_ID" = null ]; then
  echo 'FAIL: CLI did not return the created workspace ID'
  exit 1
fi
cli select-workspace --workspace "$WORKSPACE_ID" >/dev/null
for _ in $(seq 1 50); do
  if cli --json list-workspaces | jq -e --arg id "$WORKSPACE_ID" \
    '.workspaces | any(.workspace_ref == $id and .selected)' >/dev/null; then break; fi
  sleep 0.1
done
cli --json --id-format both list-panels --workspace "$WORKSPACE_ID" >"$RUN_DIR/created-panels.json"
FIRST_SURFACE="$(jq -r '.surfaces[] | select(.type == "terminal") | .surface_id' "$RUN_DIR/created-panels.json" | head -1)"
[ -n "$FIRST_SURFACE" ] || { echo 'FAIL: created workspace has no terminal surface'; exit 1; }
cli --json --id-format both new-surface --workspace "$WORKSPACE_ID" --type terminal \
  >"$RUN_DIR/second-surface.json"
SECOND_SURFACE="$(jq -r '.surface_id' "$RUN_DIR/second-surface.json")"
if [ -z "$SECOND_SURFACE" ] || [ "$SECOND_SURFACE" = null ]; then
  echo 'FAIL: CLI did not return the second terminal surface ID'
  exit 1
fi
cli focus-surface --workspace "$WORKSPACE_ID" --surface "$FIRST_SURFACE" >/dev/null

wait_for_title() {
  local title=$1
  for _ in $(seq 1 80); do
    if cli --json list-workspaces | jq -e --arg title "$title" \
      '.workspaces | any(.name == $title)' >/dev/null; then return; fi
    sleep 0.1
  done
  echo "FAIL: inline workspace rename did not commit as '$title'"
  exit 1
}

wait_for_group_title() {
  local title=$1
  for _ in $(seq 1 80); do
    if cli --json list-groups | jq -e --arg title "$title" \
      '.groups | any(.name == $title)' >/dev/null; then return; fi
    sleep 0.1
  done
  echo "FAIL: inline group rename did not commit as '$title'"
  exit 1
}

focus_external_app() {
  xdotool windowactivate --sync "$EXTERNAL_WINDOW"
  sleep 0.2
  assert_external_input_focus
  [ "$(xdotool getactivewindow)" = "$EXTERNAL_WINDOW" ] \
    || { echo 'FAIL: external X11 window did not become active'; exit 1; }
}

assert_external_input_focus() {
  local focused_name
  focused_name="$(xdotool getwindowfocus getwindowname)"
  echo "external X input focus: $focused_name"
  [ "$focused_name" = 'Limux Review Focus Target' ] \
    || { echo 'FAIL: Limux took X input focus while another app was active'; exit 1; }
}

focus_limux() {
  xdotool windowactivate --sync "$WINDOW"
  sleep 0.2
  echo "reactivated X input focus: $(xdotool getwindowfocus getwindowname)"
  [ "$(xdotool getactivewindow)" = "$WINDOW" ] \
    || { echo 'FAIL: Limux window did not reactivate'; exit 1; }
}

type_marker_command() {
  local marker=$1 proof=$2
  xdotool type --clearmodifiers --delay 5 "clear; printf '%s\\n' '$marker'; printf ok > '$proof'"
  xdotool key --clearmodifiers Return
  for _ in $(seq 1 80); do [ -f "$proof" ] && return; sleep 0.1; done
  echo "FAIL: typed command did not execute in a terminal: $marker"
  cli read-screen --workspace "$WORKSPACE_ID" --surface "$FIRST_SURFACE" || true
  cli read-screen --workspace "$WORKSPACE_ID" --surface "$SECOND_SURFACE" || true
  exit 1
}

setsid xmessage -center -title 'Limux Review Focus Target' \
  -buttons 'okay:0' 'Private test window' \
  >"$RUN_DIR/xmessage.stdout" 2>"$RUN_DIR/xmessage.stderr" &
XMESSAGE_PID=$!
EXTERNAL_WINDOW=""
for _ in $(seq 1 50); do
  EXTERNAL_WINDOW="$(xdotool search --onlyvisible --name '^Limux Review Focus Target$' 2>/dev/null | head -1 || true)"
  [ -n "$EXTERNAL_WINDOW" ] && break
  sleep 0.1
done
[ -n "$EXTERNAL_WINDOW" ] || { echo 'FAIL: xmessage window did not appear'; exit 1; }
focus_limux
sleep 0.5

# Put focus in the first terminal, then use the real sidebar gesture to begin
# editing the workspace name. X11 activation of xmessage commits the editor.
xdotool mousemove --window "$WINDOW" 1000 650 click 1
sleep 0.2
xdotool mousemove --window "$WINDOW" 100 110 click --repeat 2 --delay 100 1
sleep 0.2
xdotool key --clearmodifiers ctrl+a
xdotool type --clearmodifiers --delay 10 'Renamed while away'
if command -v import >/dev/null; then import -window root "$RUN_DIR/rename-entry.png" || true; fi
focus_external_app
wait_for_title 'Renamed while away'
assert_external_input_focus
if command -v import >/dev/null; then import -window root "$RUN_DIR/away.png" || true; fi
focus_limux
NO_CLICK_MARKER="RESTORED_WITHOUT_CLICK_$(date +%s%N)"
type_marker_command "$NO_CLICK_MARKER" "$RUN_DIR/no-click-proof"
cli read-screen --workspace "$WORKSPACE_ID" --surface "$FIRST_SURFACE" >"$RUN_DIR/first-screen.txt"
grep -Fq "$NO_CLICK_MARKER" "$RUN_DIR/first-screen.txt" \
  || { echo 'FAIL: reactivation keystrokes did not reach the previously active terminal'; exit 1; }
echo 'PASS: returning from another X11 application restores terminal input after inline rename'

# Repeat the interruption, then deliberately select and click the second tab.
# The restoration callback must leave the click target with keyboard focus.
xdotool mousemove --window "$WINDOW" 500 350 click 1
sleep 0.2
xdotool mousemove --window "$WINDOW" 100 110 click --repeat 2 --delay 100 1
sleep 0.2
xdotool key --clearmodifiers ctrl+a
xdotool type --clearmodifiers --delay 10 'Renamed again while away'
focus_external_app
wait_for_title 'Renamed again while away'
assert_external_input_focus
# Click the intended tab while xmessage still owns X input focus. This makes
# the click itself reactivate Limux and races the deferred restore callback.
xdotool mousemove --window "$WINDOW" 370 65 click 1
xdotool mousemove --window "$WINDOW" 500 350 click 1
CLICK_MARKER="CLICKED_SECOND_TERMINAL_$(date +%s%N)"
type_marker_command "$CLICK_MARKER" "$RUN_DIR/click-proof"
cli read-screen --workspace "$WORKSPACE_ID" --surface "$FIRST_SURFACE" >"$RUN_DIR/first-after-click.txt"
cli read-screen --workspace "$WORKSPACE_ID" --surface "$SECOND_SURFACE" >"$RUN_DIR/second-after-click.txt"
if ! grep -Fq "$CLICK_MARKER" "$RUN_DIR/second-after-click.txt" \
  || grep -Fq "$CLICK_MARKER" "$RUN_DIR/first-after-click.txt"; then
  echo 'FAIL: activation restoration overrode the deliberate click target'
  exit 1
fi
echo 'PASS: deliberate terminal click after reactivation retains focus'

cli --json create-group --workspace "$WORKSPACE_ID" 'Focus group' >"$RUN_DIR/focus-group.json"
for _ in $(seq 1 50); do
  if cli --json list-groups | jq -e '.groups | any(.name == "Focus group")' >/dev/null; then break; fi
  sleep 0.1
done
# Group headers use the same inline editor. Open its context menu and rename
# through the GUI so reactivation exercises that shared path too.
sleep 0.2
xdotool mousemove --window "$WINDOW" 100 110 click 3
sleep 0.3
xdotool mousemove --window "$WINDOW" 370 30 click 1
sleep 0.2
xdotool key --clearmodifiers ctrl+a
xdotool type --clearmodifiers --delay 10 'Group renamed while away'
focus_external_app
wait_for_group_title 'Group renamed while away'
assert_external_input_focus
focus_limux
GROUP_MARKER="GROUP_RESTORE_$(date +%s%N)"
type_marker_command "$GROUP_MARKER" "$RUN_DIR/group-proof"
cli read-screen --workspace "$WORKSPACE_ID" --surface "$SECOND_SURFACE" >"$RUN_DIR/group-screen.txt"
grep -Fq "$GROUP_MARKER" "$RUN_DIR/group-screen.txt" \
  || { echo 'FAIL: group rename reactivation did not restore terminal input'; exit 1; }
echo 'PASS: group inline rename also restores terminal input after reactivation'
