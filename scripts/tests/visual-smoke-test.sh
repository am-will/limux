#!/usr/bin/env bash
# Capture real Limux GTK/Ghostty UI states on an isolated X11 display.
set -euo pipefail

TOOLING_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SOURCE_ROOT="${LIMUX_TEST_ROOT:-$TOOLING_ROOT}"
HOST="${LIMUX_TEST_HOST:-$SOURCE_ROOT/target/debug/limux}"
CLI="${LIMUX_TEST_CLI:-$SOURCE_ROOT/target/debug/limux-cli}"
GHOSTTY_ROOT="$SOURCE_ROOT/ghostty/zig-out"
ARTIFACT_DIR="${LIMUX_VISUAL_SMOKE_ARTIFACT_DIR:-$TOOLING_ROOT/visual-smoke-artifacts}"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_DIR="$(cd "$ARTIFACT_DIR" && pwd)"

if [ "${1:-}" != --inside ]; then
for dependency in xvfb-run dbus-run-session xdotool metacity jq setsid import identify montage; do
  command -v "$dependency" >/dev/null || {
    echo "FAIL: required visual-smoke dependency is missing: $dependency" >&2
    exit 2
  }
done
for path in "$HOST" "$CLI" "$GHOSTTY_ROOT/lib/libghostty-internal.so" \
  "$GHOSTTY_ROOT/share/ghostty" "$GHOSTTY_ROOT/share/terminfo"; do
  [ -e "$path" ] || { echo "FAIL: required Limux runtime file is missing: $path" >&2; exit 2; }
done
if [ ! -x "$HOST" ] || [ ! -x "$CLI" ]; then
  echo "FAIL: host and CLI must be executable" >&2
  exit 2
fi

RUN_DIR="$(mktemp -d -t limux-visual-smoke-XXXXXX)"
export LIMUX_VISUAL_RUN_DIR="$RUN_DIR" LIMUX_VISUAL_ARTIFACT_DIR="$ARTIFACT_DIR"
export LIMUX_SOURCE_ROOT="$SOURCE_ROOT" LIMUX_VISUAL_HOST="$HOST" LIMUX_VISUAL_CLI="$CLI"
export XDG_DATA_HOME="$RUN_DIR/data" XDG_STATE_HOME="$RUN_DIR/state"
export XDG_CONFIG_HOME="$RUN_DIR/config" XDG_CACHE_HOME="$RUN_DIR/cache"
export XDG_RUNTIME_DIR="$RUN_DIR/runtime"
mkdir -p "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME/ghostty" \
  "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
cat >"$RUN_DIR/dbus.conf" <<'DBUS'
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
export GTK_USE_PORTAL=0 GTK_A11Y=none GIO_USE_VFS=local GSETTINGS_BACKEND=memory
# Match Limux's configured dark appearance so GTK-theme overrides do not leave
# light foreground colors on white widget backgrounds in captured screenshots.
export GDK_BACKEND=x11 GDK_SCALE=1 GTK_THEME=Adwaita:dark
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LP_NUM_THREADS=1
export SHELL=/bin/sh ENV=/dev/null PS1='' PS2=''
export LIMUX_SOCKET="$RUN_DIR/limux.sock" LIMUX_SOCKET_PATH="$RUN_DIR/limux.sock"
export LIMUX_SOCKET_MODE=localUser
export LD_LIBRARY_PATH="$GHOSTTY_ROOT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GHOSTTY_RESOURCES_DIR="$GHOSTTY_ROOT/share/ghostty"
export GHOSTTY_SHELL_INTEGRATION_XDG_DIR="$GHOSTTY_ROOT/share/ghostty/shell-integration"
export TERMINFO="$GHOSTTY_ROOT/share/terminfo"
unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DISPLAY WAYLAND_DISPLAY
unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK GPG_AGENT_INFO LIMUX_WORKSPACE_ID LIMUX_PANE_ID LIMUX_SURFACE_ID LIMUX_TAB_ID
unset XDG_ACTIVATION_TOKEN DESKTOP_STARTUP_ID LD_PRELOAD GDK_DPI_SCALE
printf 'command = /bin/sh\nshell-integration = none\nfont-size = 12\n' >"$XDG_CONFIG_HOME/ghostty/config"

result=0
xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
  dbus-run-session --config-file="$RUN_DIR/dbus.conf" -- bash "$0" --inside || result=$?
rm -rf "$RUN_DIR"
exit "$result"
fi

RUN_DIR="${LIMUX_VISUAL_RUN_DIR:?private run directory missing}"
ARTIFACT_DIR="${LIMUX_VISUAL_ARTIFACT_DIR:?artifact directory missing}"
HOST="${LIMUX_VISUAL_HOST:?host path missing}"
CLI="${LIMUX_VISUAL_CLI:?CLI path missing}"
SOURCE_ROOT="${LIMUX_SOURCE_ROOT:?source root missing}"
LOG_DIR="$ARTIFACT_DIR/logs"
mkdir -p "$LOG_DIR" "$ARTIFACT_DIR/screenshots"
HOST_PID=""
WM_PID=""
WINDOW=""
SCENARIOS="$ARTIFACT_DIR/scenarios.txt"
: >"$SCENARIOS"

cli() { "$CLI" --socket "$LIMUX_SOCKET" --json --id-format both "$@"; }
wait_window() {
  for _ in $(seq 1 300); do
    kill -0 "$HOST_PID" 2>/dev/null || { echo "FAIL: Limux host exited during startup"; return 1; }
    WINDOW="$(xdotool search --all --onlyvisible --pid "$HOST_PID" --name '^Limux v' 2>/dev/null | head -1 || true)"
    if [ -n "$WINDOW" ] && cli identify >"$LOG_DIR/identify.json" 2>/dev/null; then return 0; fi
    sleep 0.1
  done
  echo "FAIL: Limux window did not become ready"
  return 1
}
resize_window() {
  local width="$1" height="$2"
  xdotool windowmove "$WINDOW" 24 24
  xdotool windowsize --sync "$WINDOW" "$width" "$height"
  EXPECTED_DIMENSIONS="${width}x${height}"
  sleep 0.7
}
capture() {
  local name="$1" caption="$2" path="$ARTIFACT_DIR/screenshots/$1.png"
  xdotool windowmap "$WINDOW" 2>/dev/null || true
  xdotool windowraise "$WINDOW" 2>/dev/null || true
  sleep 0.25
  import -window "$WINDOW" "$path"
  identify -format '%wx%h' "$path" >"$LOG_DIR/$name.dimensions"
  [ -s "$path" ] || { echo "FAIL: screenshot is empty: $name"; return 1; }
  [ "$(cat "$LOG_DIR/$name.dimensions")" = "$EXPECTED_DIMENSIONS" ] \
    || { echo "FAIL: $name did not render at $EXPECTED_DIMENSIONS"; return 1; }
  printf '%s\t%s\t%s\n' "$name" "$caption" "screenshots/$name.png" >>"$SCENARIOS"
  echo "Captured $name ($(cat "$LOG_DIR/$name.dimensions"))"
}
wait_for_health() {
  local workspace="$1" minimum="$2" output="$3"
  for _ in $(seq 1 50); do
    if cli surface-health --workspace "$workspace" >"$output" 2>/dev/null \
      && jq -e --argjson minimum "$minimum" \
        '.surfaces | length >= $minimum and all(.[]; .type == "terminal" and .healthy == true and .realized == true and (.columns // 0) > 0 and (.rows // 0) > 0)' \
        "$output" >/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  echo "FAIL: terminal surfaces for workspace '$workspace' did not become healthy"
  cat "$output" 2>/dev/null || true
  return 1
}
write_gallery() {
  local result="$1" source_sha="${LIMUX_VISUAL_SOURCE_SHA:-unknown}" gallery="$ARTIFACT_DIR/index.html"
  {
    cat <<'HTML'
<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Limux visual smoke</title>
<style>
body{margin:0;padding:2rem;background:#12151d;color:#e8eaf0;font:16px system-ui,sans-serif}
h1{margin:0 0 .4rem}p{color:#aab1c0}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,520px),1fr));gap:1.2rem}
figure{margin:0;background:#202532;padding:.7rem;border-radius:.6rem}img{display:block;width:100%;height:auto;border-radius:.3rem}
figcaption{padding:.6rem .1rem .1rem;font-weight:600}.meta{font:13px ui-monospace,monospace;white-space:pre-wrap}
</style></head><body><h1>Limux visual smoke</h1>
HTML
    printf '<p>Result: %s<br>Source commit: %s</p><div class="grid">\n' "$result" "$source_sha"
    while IFS="$(printf '\t')" read -r name caption image_path; do
      [ -n "$name" ] || continue
      printf '<figure><a href="%s"><img loading="lazy" src="%s" alt="%s"></a><figcaption>%s</figcaption></figure>\n' \
        "$image_path" "$image_path" "$caption" "$caption"
    done <"$SCENARIOS"
    printf '</div></body></html>\n'
  } >"$gallery"
  if [ -s "$SCENARIOS" ]; then
    montage "$ARTIFACT_DIR"/screenshots/*.png -font DejaVu-Sans -pointsize 14 \
      -background '#12151d' -fill white -set label '%f' -thumbnail 640x400 \
      -tile 2x -geometry +12+28 "$ARTIFACT_DIR/contact-sheet.png" 2>"$LOG_DIR/montage.stderr" || true
  fi
}
cleanup() {
  local result=$? pid
  trap - EXIT
  set +e
  if [ "$result" -ne 0 ]; then
    echo "FAILED (exit $result)" >"$ARTIFACT_DIR/result.txt"
    if [ -n "$WINDOW" ]; then import -window "$WINDOW" "$ARTIFACT_DIR/failure-window.png" 2>/dev/null; fi
    tail -n 100 "$LOG_DIR/host.stderr" "$LOG_DIR/wm.stderr" >"$LOG_DIR/failure-tail.txt" 2>/dev/null
  else
    echo "PASSED" >"$ARTIFACT_DIR/result.txt"
  fi
  write_gallery "$(cat "$ARTIFACT_DIR/result.txt")"
  for pid in "$HOST_PID" "$WM_PID"; do
    [ -n "$pid" ] || continue
    kill -TERM -- "-$pid" 2>/dev/null
    for _ in $(seq 1 40); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL -- "-$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  done
  exit "$result"
}
trap cleanup EXIT

setsid metacity --sm-disable --no-composite >"$LOG_DIR/wm.stdout" 2>"$LOG_DIR/wm.stderr" &
WM_PID=$!
for _ in $(seq 1 50); do
  kill -0 "$WM_PID" 2>/dev/null || { echo "FAIL: Metacity exited during startup"; exit 1; }
  xdotool get_desktop >/dev/null 2>&1 && break
  sleep 0.1
done
xdotool get_desktop >/dev/null
setsid "$HOST" >"$LOG_DIR/host.stdout" 2>"$LOG_DIR/host.stderr" &
HOST_PID=$!
wait_window

ORIGINAL_ID="$(jq -r '.focused.workspace_id // .focused.workspace_ref // empty' "$LOG_DIR/identify.json")"
[ -n "$ORIGINAL_ID" ] || { echo "FAIL: identify returned no focused workspace id"; exit 1; }
cli list-workspaces >"$LOG_DIR/workspaces-initial.json"
wait_for_health "$ORIGINAL_ID" 1 "$LOG_DIR/initial-health.json"
resize_window 1320 820
capture default-wide 'Default workspace with an empty terminal'

NEW_JSON="$(cli new-workspace --cwd "$RUN_DIR")"
VISUAL_ID="$(jq -r '.workspace_id // .workspace_ref // empty' <<<"$NEW_JSON")"
[ -n "$VISUAL_ID" ] || { echo "FAIL: new-workspace returned no workspace id"; exit 1; }
cli rename-workspace --workspace "$VISUAL_ID" 'Visual Smoke Workspace' >/dev/null
cli select-workspace --workspace "$VISUAL_ID" >/dev/null
SURFACE="$(cli list-panels --workspace "$VISUAL_ID" | jq -r '.surfaces[0].surface_id // .surfaces[0].surface_ref // empty')"
[ -n "$SURFACE" ] || { echo "FAIL: new workspace has no terminal surface"; exit 1; }
wait_for_health "$VISUAL_ID" 1 "$LOG_DIR/surface-health.json"
cli send --workspace "$VISUAL_ID" --surface "$SURFACE" "printf 'LIMUX_VISUAL_POPULATED\\n'" >/dev/null
cli send-key --workspace "$VISUAL_ID" --surface "$SURFACE" Enter >/dev/null
for _ in $(seq 1 50); do
  if cli read-screen --workspace "$VISUAL_ID" --surface "$SURFACE" >"$LOG_DIR/populated-screen.json" 2>/dev/null; then
    jq -r '.text' "$LOG_DIR/populated-screen.json" >"$LOG_DIR/populated-screen.txt"
  fi
  grep -Fxq 'LIMUX_VISUAL_POPULATED' "$LOG_DIR/populated-screen.txt" 2>/dev/null && break
  sleep 0.1
done
grep -Fxq 'LIMUX_VISUAL_POPULATED' "$LOG_DIR/populated-screen.txt"
resize_window 1320 820
capture populated-wide 'Workspace with live terminal output'

PANE_JSON="$(cli new-pane --workspace "$VISUAL_ID" --surface "$SURFACE" --direction right)"
PANE_SURFACE="$(jq -r '.surface_id // .surface_ref // empty' <<<"$PANE_JSON")"
[ -n "$PANE_SURFACE" ] || { echo "FAIL: new-pane returned no surface id"; exit 1; }
wait_for_health "$VISUAL_ID" 2 "$LOG_DIR/split-health.json"
resize_window 1320 820
capture split-wide 'Two live terminal panes'
resize_window 790 820
capture split-narrow 'Two panes in a narrow window'

# Group screenshots are feature-detected through the running host bridge. Main
# currently has no group commands, while PRs that add them render both states.
if "$CLI" --help 2>&1 | grep -Eq '(^|[[:space:],])list-groups([[:space:],]|$)'; then
  cli list-groups >"$LOG_DIR/groups-before.json"
  GROUP_JSON="$(cli create-group --color blue --workspace "$VISUAL_ID" 'Visual Smoke Group')"
  GROUP_ID="$(jq -r '.group_id // .id // empty' <<<"$GROUP_JSON")"
  [ -n "$GROUP_ID" ] || { echo "FAIL: create-group returned no group id"; exit 1; }
  cli list-groups >"$LOG_DIR/groups-expanded.json"
  jq -e --arg id "$GROUP_ID" '.groups | any(.[]; (.group_id // .id) == $id and .collapsed == false)' \
    "$LOG_DIR/groups-expanded.json" >/dev/null
  cli select-workspace --workspace "$ORIGINAL_ID" >/dev/null
  resize_window 1320 820
  capture groups-expanded 'Expanded workspace group in the sidebar'
  cli collapse-group --group "$GROUP_ID" >/dev/null
  cli list-groups >"$LOG_DIR/groups-collapsed.json"
  jq -e --arg id "$GROUP_ID" '.groups | any(.[]; (.group_id // .id) == $id and .collapsed == true)' \
    "$LOG_DIR/groups-collapsed.json" >/dev/null
  capture groups-collapsed 'Collapsed workspace group in the sidebar'
else
  echo 'Workspace groups unsupported by this source; optional group screenshots skipped.' \
    | tee "$LOG_DIR/groups-skipped.txt"
fi

SOURCE_SHA="${LIMUX_VISUAL_SOURCE_SHA:-$(git -C "$SOURCE_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}"
printf 'Result: PASS\nSource: %s\nScenarios:\n' "$SOURCE_SHA" >"$ARTIFACT_DIR/summary.txt"
cut -f1,2 "$SCENARIOS" >>"$ARTIFACT_DIR/summary.txt"
echo "PASS: captured Limux visual states at $ARTIFACT_DIR"
