#!/usr/bin/env bash
# Exercise real GTK tab/split actions in an isolated X11 session.
# Requires prebuilt host/CLI, Xvfb, xdotool, dbus-run-session, and jq.
# LIMUX_TEST_PROFILE=release selects release binaries; LIMUX_TEST_HOST and
# LIMUX_TEST_CLI can point to another checkout for a before/after comparison.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
"$ROOT_DIR/scripts/check-ghostty.sh"
for dependency in xvfb-run xdotool dbus-run-session jq setsid; do
  command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency"; exit 2; }
done
if [ "${1:-}" != --inside ]; then
  LIMUX_CWD_TEST_DIR="$(mktemp -d -t limux-terminal-cwd-XXXXXX)"
  export LIMUX_CWD_TEST_DIR
  export XDG_DATA_HOME="$LIMUX_CWD_TEST_DIR/data" XDG_STATE_HOME="$LIMUX_CWD_TEST_DIR/state"
  export XDG_CONFIG_HOME="$LIMUX_CWD_TEST_DIR/config" XDG_RUNTIME_DIR="$LIMUX_CWD_TEST_DIR/runtime"
  export XDG_CACHE_HOME="$LIMUX_CWD_TEST_DIR/cache"
  mkdir -p "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME/ghostty" \
    "$XDG_RUNTIME_DIR" "$XDG_CACHE_HOME" "$LIMUX_CWD_TEST_DIR/workspace/nested" "$LIMUX_CWD_TEST_DIR/other/nested"
  chmod 700 "$XDG_RUNTIME_DIR"
  export GTK_USE_PORTAL=0 GIO_USE_VFS=local GTK_A11Y=none
  unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE WAYLAND_DISPLAY
  unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK GPG_AGENT_INFO
  # Do not activate desktop services that can escape the private XDG paths.
  printf '%s\n' '<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>' \
    >"$LIMUX_CWD_TEST_DIR/dbus.conf"
  exec xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
    dbus-run-session --config-file="$LIMUX_CWD_TEST_DIR/dbus.conf" -- bash "$0" --inside
fi

PROFILE="${LIMUX_TEST_PROFILE:-debug}"
HOST="${LIMUX_TEST_HOST:-$ROOT_DIR/target/$PROFILE/limux}"
CLI="${LIMUX_TEST_CLI:-$ROOT_DIR/target/$PROFILE/limux-cli}"
if [ ! -x "$HOST" ] || [ ! -x "$CLI" ]; then echo "Build the host and CLI first"; exit 2; fi
RUN_DIR="${LIMUX_CWD_TEST_DIR:?}"
echo "Test artifacts: $RUN_DIR"
export LIMUX_SOCKET="$RUN_DIR/limux.sock" LIMUX_SOCKET_PATH="$RUN_DIR/limux.sock"
export LIMUX_SOCKET_MODE=runtime GDK_BACKEND=x11 GDK_SCALE=1 GTK_THEME=Adwaita
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LP_NUM_THREADS=1 SHELL=/bin/sh
export GTK_USE_PORTAL=0 GTK_A11Y=none GIO_USE_VFS=local
export LD_LIBRARY_PATH="$ROOT_DIR/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GHOSTTY_RESOURCES_DIR="$ROOT_DIR/ghostty/zig-out/share/ghostty"
export TERMINFO="$ROOT_DIR/ghostty/zig-out/share/terminfo"
unset LIMUX_WORKSPACE_ID LIMUX_PANE_ID LIMUX_SURFACE_ID LIMUX_TAB_ID WAYLAND_DISPLAY
unset LD_PRELOAD GDK_DPI_SCALE
printf 'command = /bin/sh\nshell-integration = none\nfont-size = 12\n' \
  >"$XDG_CONFIG_HOME/ghostty/config"
jq -n --arg base "$RUN_DIR" '{
  version: 1, active_workspace_index: 0, top_bar_visible: true,
  sidebar: {visible: false, width: 300},
  workspaces: [
    {id: "cwd-main", name: "cwd-main", folder_path: ($base + "/workspace"),
     autostart_command: ("printf '\''%s\\n'\'' \"$LIMUX_SURFACE_ID\" >> " + $base + "/autostart.log"),
     cwd: ($base + "/workspace"), layout: {kind: "pane", pane_id: 1,
     active_tab_id: "main", tabs: [{id: "main", tab_kind: "terminal", cwd: ($base + "/workspace")}] }},
    {id: "cwd-other", name: "cwd-other", folder_path: ($base + "/other"),
     cwd: ($base + "/other"), layout: {kind: "split", orientation: "horizontal", ratio: 0.5,
       start: {kind: "pane", pane_id: 6, active_tab_id: "other-left",
         tabs: [{id: "other-left", tab_kind: "terminal", cwd: ($base + "/other/nested")}]},
       end: {kind: "pane", pane_id: 2, active_tab_id: "other-right",
         tabs: [{id: "other-right", tab_kind: "terminal", cwd: ($base + "/other/nested")}]}
     }}
  ]
}' >"$XDG_DATA_HOME/limux/session.json"

HOST_PID=""
cleanup() {
  local result=$?
  if [ "$result" -ne 0 ] && command -v import >/dev/null; then
    import -window root "$RUN_DIR/window.png" || true
  fi
  if [ -n "$HOST_PID" ]; then
    kill -TERM -- "-$HOST_PID" 2>/dev/null || true
    wait "$HOST_PID" 2>/dev/null || true
  fi
  if [ "$result" -ne 0 ]; then tail -40 "$RUN_DIR/host.stderr"; fi
}
trap cleanup EXIT
setsid "$HOST" >"$RUN_DIR/host.stdout" 2>"$RUN_DIR/host.stderr" &
HOST_PID=$!
WINDOW=""
for _ in $(seq 1 450); do
  kill -0 "$HOST_PID" || { echo "Host exited during startup"; exit 1; }
  WINDOW="$(xdotool search --onlyvisible --pid "$HOST_PID" 2>/dev/null | head -1 || true)"
  [ -n "$WINDOW" ] && [ -S "$LIMUX_SOCKET" ] && break
  sleep 0.1
done
if [ -z "$WINDOW" ] || [ ! -S "$LIMUX_SOCKET" ]; then echo "Host startup timed out"; exit 1; fi
xdotool windowsize --sync "$WINDOW" 1200 760 windowfocus --sync "$WINDOW"
sleep 1

wait_for_count() {
  local workspace=$1 expected=$2 expected_type=${3:-terminal}
  local surface_id surface_type
  for _ in $(seq 1 100); do
    "$CLI" --json --id-format both list-panels --workspace "$workspace" \
      >"$RUN_DIR/surfaces.json"
    if [ "$(jq '.surfaces | length' "$RUN_DIR/surfaces.json")" -eq "$expected" ]; then
      if [ "$expected_type" = browser ] \
        && jq -e '.surfaces[] | select(.type == "browser" and .selected)' \
          "$RUN_DIR/surfaces.json" >/dev/null; then return; fi
      surface_id="$(jq -r '.surfaces[] | select(.focused) | .surface_id' "$RUN_DIR/surfaces.json")"
      surface_type="$(jq -r '.surfaces[] | select(.focused) | .type' "$RUN_DIR/surfaces.json")"
      if [ "$surface_type" = terminal ] \
        && "$CLI" --json --id-format both surface-health --workspace "$workspace" \
          >"$RUN_DIR/health.json" \
        && jq -e --arg id "$surface_id" '.surfaces[] | select(.surface_id == $id) |
          .healthy and .realized and (.process_exited == false) and
          .columns > 0 and .rows > 0' "$RUN_DIR/health.json" >/dev/null; then return; fi
    fi
    sleep 0.1
  done
  echo "FAIL: $workspace expected $expected surfaces with a ready focused target"
  exit 1
}
assert_directory() {
  local expected=$1 label=$2 workspace=${3:-cwd-main}
  local proof="$RUN_DIR/$label.pwd"
  "$CLI" send --workspace "$workspace" "pwd > '$proof'" >/dev/null
  "$CLI" send-key --workspace "$workspace" Enter >/dev/null
  for _ in $(seq 1 50); do [ -s "$proof" ] && break; sleep 0.1; done
  if [ ! -s "$proof" ] || [ "$(<"$proof")" != "$expected" ]; then
    echo "FAIL: $label expected $expected, got $(cat "$proof" 2>/dev/null || true)"
    exit 1
  fi
  echo "PASS: $label"
}
set_directory() {
  local directory=$1
  "$CLI" send --workspace cwd-main \
    "cd '$directory'; printf '\033]7;file://localhost%s\007' \"\$PWD\"" >/dev/null
  "$CLI" send-key --workspace cwd-main Enter >/dev/null
  for _ in $(seq 1 50); do
    if "$CLI" --json list-panels --workspace cwd-main \
      | jq -e --arg cwd "$directory" '.surfaces[] | select(.focused and .cwd == $cwd)' >/dev/null; then return; fi
    sleep 0.1
  done
  echo "FAIL: OSC 7 directory was not tracked"
  exit 1
}
key() { xdotool key --clearmodifiers "$1"; }
click() { xdotool mousemove --window "$WINDOW" "$1" "$2" click "${3:-1}"; }

wait_for_sidebar() {
  local visible=$1 label=${2:-sidebar}
  for _ in $(seq 1 50); do
    if jq -e --argjson visible "$visible" \
      '.sidebar.visible == $visible and .sidebar.width == 300' \
      "$XDG_DATA_HOME/limux/session.json" >/dev/null; then
      echo "PASS: $label visible=$visible preserves restored width"
      return
    fi
    sleep 0.1
  done
  echo "FAIL: $label visible=$visible with restored width 300 was not persisted"
  exit 1
}

rapid_sidebar_clicks() {
  local count=$1 visible=$2
  xdotool mousemove --window "$WINDOW" 20 25 click --repeat "$count" --delay 50 1
  # Initial and final state can match; wait for animation/save, not stale JSON.
  sleep 0.6
  wait_for_sidebar "$visible" "sidebar after $count rapid clicks"
}

wait_for_count cwd-main 1
assert_directory "$RUN_DIR/workspace" initial-directory
wait_for_sidebar false
click 20 25
wait_for_sidebar true
key ctrl+alt+m
wait_for_sidebar false
key ctrl+alt+m
wait_for_sidebar true
click 20 25
wait_for_sidebar false
click 20 25
wait_for_sidebar true

rapid_sidebar_clicks 2 true
rapid_sidebar_clicks 3 false
rapid_sidebar_clicks 2 false
rapid_sidebar_clicks 3 true

set_directory "$RUN_DIR/workspace/nested"
key ctrl+shift+t
wait_for_count cwd-main 2
assert_directory "$RUN_DIR/workspace/nested" new-tab-shortcut
click 1072 51
wait_for_count cwd-main 3
assert_directory "$RUN_DIR/workspace/nested" new-tab-button
key ctrl+alt+d
wait_for_count cwd-main 4
assert_directory "$RUN_DIR/workspace/nested" split-shortcut

# Browser focus has no cwd. In a newly inherited split, fallback must still
# be the workspace root, not the first terminal's inherited directory.
click 1099 51
wait_for_count cwd-main 5 browser
click 1072 51
wait_for_count cwd-main 6
assert_directory "$RUN_DIR/workspace" browser-fallback-after-split
set_directory "$RUN_DIR/workspace/nested"
click 1153 51
wait_for_count cwd-main 7
assert_directory "$RUN_DIR/workspace/nested" split-button
if [ "$(wc -l <"$RUN_DIR/autostart.log")" -ne 6 ] \
  || [ "$(sort -u "$RUN_DIR/autostart.log" | wc -l)" -ne 6 ]; then
  echo "FAIL: workspace autostart did not run exactly once per terminal"
  exit 1
fi

# On the active workspace, keep the pane that had focus before the menu.
PREFERRED_PANE="$("$CLI" --json --id-format both identify | jq -r '.focused.pane_id')"
click 70 65 3
sleep 0.2
# GTK focuses the first action, regardless of where the popover fits on screen.
key Return
wait_for_count cwd-main 8
assert_directory "$RUN_DIR/workspace" active-workspace-context-root
"$CLI" --json --id-format both identify \
  | jq -e --arg pane "$PREFERRED_PANE" '.focused.pane_id == $pane' >/dev/null \
  || { echo "FAIL: context action did not preserve the focused pane"; exit 1; }

# The sidebar action explicitly ignores terminal cwd, including when its
# workspace was inactive and restores a terminal in a nested directory.
click 70 135 3
sleep 0.2
key Return
wait_for_count cwd-other 3
assert_directory "$RUN_DIR/other" workspace-context-root cwd-other
"$CLI" --json --id-format both identify >"$RUN_DIR/context-focus.json"
jq -e '.focused.name == "cwd-other" and .focused.pane_id == "6"' "$RUN_DIR/context-focus.json" >/dev/null \
  || { echo "FAIL: context action did not activate its workspace"; exit 1; }

# When the top bar is hidden, the sidebar toggle moves with the visible pane.
# Moving it must not let a non-resize drag overwrite the saved sidebar width.
click 950 200
key ctrl+alt+shift+m
sleep 0.6
key ctrl+alt+m
sleep 0.6
wait_for_sidebar false
key ctrl+shift+z
sleep 0.6
click 20 20
sleep 0.6
wait_for_sidebar true dock-after-zoom
key ctrl+alt+m
sleep 0.6
wait_for_sidebar false
key ctrl+shift+z
sleep 0.6
click 20 20
sleep 0.6
wait_for_sidebar true dock-after-unzoom
key ctrl+alt+m
sleep 0.6
key ctrl+alt+Page_Up
sleep 0.6
click 20 20
sleep 0.6
wait_for_sidebar true dock-after-workspace-switch

# Empty minimal mode must still expose the sidebar and its New Workspace button.
key ctrl+alt+m
sleep 0.6
# The GUI permits closing the final workspace; the control API deliberately does not.
key ctrl+alt+shift+w
sleep 0.6
key ctrl+alt+shift+w
sleep 0.6
"$CLI" --json list-workspaces | jq -e '.workspaces | length == 0' >/dev/null
sleep 0.6
click 20 20
sleep 0.6
wait_for_sidebar true dock-without-workspaces
click 74 20
DIALOG=""
for _ in $(seq 1 50); do
  DIALOG="$(xdotool search --onlyvisible --name '^Open Folder as Workspace$' 2>/dev/null | head -1 || true)"
  [ -n "$DIALOG" ] && break
  sleep 0.1
done
[ -n "$DIALOG" ] || { echo 'FAIL: New Workspace did not open its folder dialog'; exit 1; }
xdotool windowfocus --sync "$DIALOG"
key ctrl+a
xdotool type --clearmodifiers --delay 10 "$RUN_DIR/workspace"
key Return
sleep 0.6
RESTORED_WORKSPACE="$("$CLI" --json --id-format both identify | jq -r '.focused.workspace_id')"
wait_for_count "$RESTORED_WORKSPACE" 1
assert_directory "$RUN_DIR/workspace" restored-workspace-directory "$RESTORED_WORKSPACE"
key ctrl+alt+m
sleep 0.6
click 20 20
sleep 0.6
wait_for_sidebar true dock-after-workspace-recreation

echo "Terminal cwd regression checks passed"
