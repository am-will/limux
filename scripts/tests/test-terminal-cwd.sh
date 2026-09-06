#!/usr/bin/env bash
# Exercise real GTK tab/split actions in an isolated X11 session.
# Requires prebuilt host/CLI, Xvfb, xdotool, dbus-run-session, and jq.
# LIMUX_TEST_PROFILE=release selects release binaries; LIMUX_TEST_HOST and
# LIMUX_TEST_CLI can point to another checkout for a before/after comparison.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
for dependency in xvfb-run xdotool dbus-run-session jq setsid; do
  command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency"; exit 2; }
done
if [ "${1:-}" != --inside ]; then
  exec xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
    dbus-run-session -- bash "$0" --inside
fi

PROFILE="${LIMUX_TEST_PROFILE:-debug}"
HOST="${LIMUX_TEST_HOST:-$ROOT_DIR/target/$PROFILE/limux}"
CLI="${LIMUX_TEST_CLI:-$ROOT_DIR/target/$PROFILE/limux-cli}"
if [ ! -x "$HOST" ] || [ ! -x "$CLI" ]; then echo "Build the host and CLI first"; exit 2; fi
RUN_DIR="$(mktemp -d -t limux-terminal-cwd-XXXXXX)"
echo "Test artifacts: $RUN_DIR"
export XDG_DATA_HOME="$RUN_DIR/data" XDG_STATE_HOME="$RUN_DIR/state"
export XDG_CONFIG_HOME="$RUN_DIR/config" XDG_RUNTIME_DIR="$RUN_DIR/runtime"
mkdir -p "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME/ghostty" \
  "$XDG_RUNTIME_DIR" "$RUN_DIR/workspace/nested" "$RUN_DIR/other/nested"
chmod 700 "$XDG_RUNTIME_DIR"
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
  sidebar: {visible: true, width: 220},
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
# Keep split ratios at exactly 0.5 to isolate cwd behavior from the existing
# session-save conflict caused by fractional-ratio JSON round trips.
xdotool windowsize --sync "$WINDOW" 1201 760 windowfocus --sync "$WINDOW"
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

wait_for_count cwd-main 1
assert_directory "$RUN_DIR/workspace" initial-directory
set_directory "$RUN_DIR/workspace/nested"
key ctrl+shift+t
wait_for_count cwd-main 2
assert_directory "$RUN_DIR/workspace/nested" new-tab-shortcut
click 1045 65
wait_for_count cwd-main 3
assert_directory "$RUN_DIR/workspace/nested" new-tab-button
key ctrl+alt+d
wait_for_count cwd-main 4
assert_directory "$RUN_DIR/workspace/nested" split-shortcut

# Browser focus has no cwd. In a newly inherited split, fallback must still
# be the workspace root, not the first terminal's inherited directory.
click 1072 65
wait_for_count cwd-main 5 browser
click 1045 65
wait_for_count cwd-main 6
assert_directory "$RUN_DIR/workspace" browser-fallback-after-split
set_directory "$RUN_DIR/workspace/nested"
click 1126 65
wait_for_count cwd-main 7
assert_directory "$RUN_DIR/workspace/nested" split-button
if [ "$(wc -l <"$RUN_DIR/autostart.log")" -ne 6 ] \
  || [ "$(sort -u "$RUN_DIR/autostart.log" | wc -l)" -ne 6 ]; then
  echo "FAIL: workspace autostart did not run exactly once per terminal"
  exit 1
fi

# On the active workspace, keep the pane that had focus before the menu.
PREFERRED_PANE="$("$CLI" --json --id-format both identify | jq -r '.focused.pane_id')"
click 70 120 3
sleep 0.2
click 360 65
wait_for_count cwd-main 8
assert_directory "$RUN_DIR/workspace" active-workspace-context-root
"$CLI" --json --id-format both identify \
  | jq -e --arg pane "$PREFERRED_PANE" '.focused.pane_id == $pane' >/dev/null \
  || { echo "FAIL: context action did not preserve the focused pane"; exit 1; }

# The sidebar action explicitly ignores terminal cwd, including when its
# workspace was inactive and restores a terminal in a nested directory.
click 70 185 3
sleep 0.2
click 360 135
wait_for_count cwd-other 3
assert_directory "$RUN_DIR/other" workspace-context-root cwd-other
"$CLI" --json --id-format both identify >"$RUN_DIR/context-focus.json"
jq -e '.focused.name == "cwd-other" and .focused.pane_id == "6"' "$RUN_DIR/context-focus.json" >/dev/null \
  || { echo "FAIL: context action did not activate its workspace"; exit 1; }
echo "Terminal cwd regression checks passed"
