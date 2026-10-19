#!/usr/bin/env bash
# Validate an installed development Flatpak without touching the user's session.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
for dependency in flatpak xvfb-run dbus-run-session jq xdotool rg tput setsid git; do
  command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency" >&2; exit 2; }
done
if [ "${1:-}" != --inside ]; then
  RUN_DIR="$(mktemp -d "$HOME/.cache/limux-flatpak-runtime-XXXXXX")"
  export RUN_DIR
  export FLATPAK_USER_DIR="${FLATPAK_USER_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/flatpak}"
  export HOME="$RUN_DIR/home"
  export XDG_CONFIG_HOME="$RUN_DIR/config" XDG_DATA_HOME="$RUN_DIR/data"
  export XDG_STATE_HOME="$RUN_DIR/state" XDG_CACHE_HOME="$RUN_DIR/cache" XDG_RUNTIME_DIR="$RUN_DIR/runtime"
  export LIMUX_AGENT_HOOK_STATE_DIR="$RUN_DIR/agent-state"
  mkdir -p "$HOME" "$XDG_CONFIG_HOME/ghostty" "$XDG_CONFIG_HOME/zsh" "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR" "$RUN_DIR/services" "$RUN_DIR/project" "$RUN_DIR/host-bin" "$LIMUX_AGENT_HOOK_STATE_DIR"
  export PATH="$RUN_DIR/host-bin:$PATH"
  cat >"$HOME/.profile" <<'PROFILE'
export PATH="$LIMUX_FLATPAK_TEST_DIR/host-bin:$PATH"
PROFILE
  cp "$HOME/.profile" "$XDG_CONFIG_HOME/zsh/.zprofile"
  printf '#!/bin/sh\nexit 99\n' >"$RUN_DIR/host-bin/limux"
  chmod 755 "$RUN_DIR/host-bin/limux"
  chmod 700 "$XDG_RUNTIME_DIR"
  SESSION_HELPER="${LIMUX_FLATPAK_SESSION_HELPER:-}"
  if [ -z "$SESSION_HELPER" ]; then
    for candidate in /usr/libexec/flatpak-session-helper /usr/lib/flatpak-session-helper /usr/lib/flatpak/flatpak-session-helper; do
      if [ -x "$candidate" ]; then SESSION_HELPER="$candidate"; break; fi
    done
  fi
  [ -x "$SESSION_HELPER" ] || { echo 'Flatpak session helper was not found' >&2; exit 2; }
  cat >"$RUN_DIR/services/org.freedesktop.Flatpak.service" <<SERVICE
[D-BUS Service]
Name=org.freedesktop.Flatpak
Exec=$SESSION_HELPER
SERVICE
  cat >"$RUN_DIR/dbus.conf" <<DBUS
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <servicedir>$RUN_DIR/services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
DBUS
  unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DISPLAY WAYLAND_DISPLAY
  unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK GPG_AGENT_INFO LD_PRELOAD TERMINFO TERMINFO_DIRS
  unset LIMUX_WORKSPACE_ID LIMUX_SURFACE_ID LIMUX_PANE_ID LIMUX_TAB_ID
  unset CODEX_HOME CLAUDE_CONFIG_DIR OPENCODE_CONFIG_DIR
  export SHELL="${LIMUX_FLATPAK_TEST_SHELL:-/bin/sh}" ENV=/dev/null HISTFILE=/dev/null
  export ZDOTDIR="$XDG_CONFIG_HOME/zsh"
  printf '%s\n' "PS1='host-test%% '" >"$ZDOTDIR/.zshrc"
  exec xvfb-run -a -s '-screen 0 1440x1000x24 -nolisten tcp' \
    dbus-run-session --config-file="$RUN_DIR/dbus.conf" -- bash "$0" --inside
fi
CLI="${LIMUX_TEST_CLI:-$ROOT_DIR/target/debug/limux-cli}"
[ -x "$CLI" ] || { echo "Build limux-cli first, or set LIMUX_TEST_CLI" >&2; exit 2; }
SOCKET="$RUN_DIR/limux.sock"
HOST_PID=""
FIXTURE="$(mktemp -t limux-flatpak-host-fixture-XXXXXX)"
RESTORED_AGENT="$(mktemp -t limux-flatpak-restored-agent-XXXXXX)"
cleanup() {
  local result=$?
  if [ "$result" -ne 0 ]; then tail -n 60 "$RUN_DIR/host.stderr" 2>/dev/null || true; fi
  if [ -n "$HOST_PID" ]; then
    kill -TERM -- "-$HOST_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$HOST_PID" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$HOST_PID" 2>/dev/null; then kill -KILL -- "-$HOST_PID" 2>/dev/null || true; fi
    wait "$HOST_PID" 2>/dev/null || true
  fi
  rm -f "$FIXTURE" "$RESTORED_AGENT"
}
trap cleanup EXIT
cli() { "$CLI" --socket "$SOCKET" --id-format both "$@"; }
flatpak_args=(--user run --socket=x11 --nosocket=wayland
  "--env=XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" "--env=LIMUX_SOCKET=$SOCKET"
  "--env=LIMUX_SOCKET_PATH=$SOCKET" --env=LIMUX_SOCKET_MODE=localUser
  --env=GTK_A11Y=none --env=GTK_USE_PORTAL=0 --env=GSETTINGS_BACKEND=memory
  --env=GDK_BACKEND=x11 --env=GIO_USE_VFS=local --env=LIBGL_ALWAYS_SOFTWARE=1
  --env=GALLIUM_DRIVER=llvmpipe --env=LP_NUM_THREADS=1
  "--env=LIMUX_AGENT_HOOK_STATE_DIR=$LIMUX_AGENT_HOOK_STATE_DIR"
  "--env=LIMUX_FLATPAK_TEST_DIR=$RUN_DIR" "--env=LIMUX_FLATPAK_TEST_CLI=$CLI")
private_env=("HOME=$HOME" "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" "XDG_DATA_HOME=$XDG_DATA_HOME"
  "XDG_STATE_HOME=$XDG_STATE_HOME" "XDG_CACHE_HOME=$XDG_CACHE_HOME")
printf 'shell-integration = %s\nfont-size = 12\n' "${LIMUX_FLATPAK_TEST_INTEGRATION:-none}" >"$XDG_CONFIG_HOME/ghostty/config"
if [ -n "${LIMUX_FLATPAK_TEST_COMMAND:-}" ]; then
  printf 'command = %s\n' "$LIMUX_FLATPAK_TEST_COMMAND" >>"$XDG_CONFIG_HOME/ghostty/config"
fi
cat >"$RESTORED_AGENT" <<'AGENT'
#!/bin/sh
set -eu
[ ! -e /.flatpak-info ]
[ "$1" = --resume ] && [ "$2" = flatpak-restore-session ]
printf restored >"$LIMUX_FLATPAK_TEST_DIR/restored-agent-ran"
attempt=0
while [ ! -f "$LIMUX_FLATPAK_TEST_DIR/release-restored-agent" ]; do
  [ "$attempt" -lt 200 ] || exit 1
  sleep 0.1
  attempt=$((attempt + 1))
done
AGENT
chmod 755 "$RESTORED_AGENT"
ln -s "$RESTORED_AGENT" "$RUN_DIR/host-bin/claude"
jq -n --arg cwd "$RUN_DIR/project" --arg executable "$RESTORED_AGENT" '{version: 1, active_workspace_index: 0,
  top_bar_visible: true, sidebar: {visible: true, width: 220},
  workspaces: [{id: "00000000-0000-4000-8000-000000000197", name: "Flatpak host shell", cwd: $cwd,
    layout: {kind: "pane", pane_id: 1, active_tab_id: "flatpak-tab", tabs: [
      {id: "flatpak-tab", tab_kind: "terminal", cwd: $cwd, agent: {
        kind: "claude", session_id: "flatpak-restore-session", cwd: $cwd, restore_on_startup: true,
        launch_command: {executable: $executable, arguments: [$executable], cwd: $cwd}
      }}
    ]}}]
}' >"$XDG_DATA_HOME/limux/session.json"
jq -n --arg cwd "$RUN_DIR/project" --arg executable "$RESTORED_AGENT" '{version: 1, sessions: {"flatpak-restore-session": {
  session_id: "flatpak-restore-session", workspace_id: "00000000-0000-4000-8000-000000000197",
  surface_id: "1:flatpak-tab", cwd: $cwd, updated_at: 1,
  launch_command: {executable: $executable, arguments: [$executable], cwd: $cwd, captured_at: 1}
}}}' >"$LIMUX_AGENT_HOOK_STATE_DIR/claude-hook-sessions.json"
cat >"$FIXTURE" <<'FIXTURE'
#!/bin/sh
set -eu
[ ! -e /.flatpak-info ]
[ -t 0 ] && [ -t 1 ]
[ "$PWD" = "$LIMUX_FLATPAK_TEST_DIR/project" ]
[ "$TERM" = xterm-256color ]
[ -n "$LIMUX_WORKSPACE_ID" ] && [ -n "$LIMUX_PANE_ID" ]
[ -n "$LIMUX_TAB_ID" ] && [ -n "$LIMUX_SURFACE_ID" ]
[ "$LIMUX_SOCKET" = "$LIMUX_FLATPAK_TEST_DIR/limux.sock" ]
printf '%s\n' "$PWD" >"$LIMUX_FLATPAK_TEST_DIR/host-pwd"
tty >"$LIMUX_FLATPAK_TEST_DIR/host-tty"
tput colors >"$LIMUX_FLATPAK_TEST_DIR/host-colors"
command -v git >"$LIMUX_FLATPAK_TEST_DIR/host-git-path"
git --version >"$LIMUX_FLATPAK_TEST_DIR/host-git-version"
"$LIMUX_FLATPAK_TEST_CLI" --socket "$LIMUX_SOCKET" --id-format both --json identify \
  --workspace "$LIMUX_WORKSPACE_ID" --surface "$LIMUX_SURFACE_ID" >"$LIMUX_FLATPAK_TEST_DIR/host-identify.json"
printf 'FLATPAK_HOST_READY\n'
printf 'cwd: %s\n' "$PWD"
printf 'tty: %s; TERM=%s; colors=%s\n' "$(tty)" "$TERM" "$(tput colors)"
git --version
printf ready >"$LIMUX_FLATPAK_TEST_DIR/ready"
IFS= read -r answer
[ "$answer" = "interactive host input" ]
printf 'FLATPAK_HOST_OK\n'
printf ok >"$LIMUX_FLATPAK_TEST_DIR/proof"
FIXTURE
chmod 755 "$FIXTURE"
# Flatpak rewrites XDG home variables after --env; set them inside the sandbox.
setsid flatpak "${flatpak_args[@]}" --command=env dev.limux.linux \
  "${private_env[@]}" /app/bin/limux >"$RUN_DIR/host.stdout" 2>"$RUN_DIR/host.stderr" &
HOST_PID=$!
for _ in $(seq 1 150); do
  if [ -S "$SOCKET" ] && cli --json list-workspaces >"$RUN_DIR/workspaces.json" 2>/dev/null; then break; fi
  kill -0 "$HOST_PID" || { echo 'Flatpak exited during startup' >&2; exit 1; }
  sleep 0.2
done
[ -S "$SOCKET" ] || { echo 'Flatpak control socket did not appear' >&2; exit 1; }
WORKSPACE="$(jq -r '.workspaces[0].workspace_id' "$RUN_DIR/workspaces.json")"
cli --json list-panels --workspace "$WORKSPACE" >"$RUN_DIR/panes.json"
SURFACE="$(jq -r '.surfaces[0].surface_id' "$RUN_DIR/panes.json")"
for _ in $(seq 1 100); do [ -f "$RUN_DIR/restored-agent-ran" ] && break; sleep 0.1; done
[ -f "$RUN_DIR/restored-agent-ran" ] || { echo 'Restored agent did not run on the host' >&2; exit 1; }
touch "$RUN_DIR/release-restored-agent"
for _ in $(seq 1 100); do
  if jq -e '.sessions | has("flatpak-restore-session") | not' "$LIMUX_AGENT_HOOK_STATE_DIR/claude-hook-sessions.json" >/dev/null; then break; fi
  sleep 0.1
done
jq -e '.sessions | has("flatpak-restore-session") | not' "$LIMUX_AGENT_HOOK_STATE_DIR/claude-hook-sessions.json" >/dev/null \
  || { echo 'Restored agent cleanup did not remove its binding' >&2; exit 1; }
cp "$LIMUX_AGENT_HOOK_STATE_DIR/claude-hook-sessions.json" "$RUN_DIR/restored-cleanup.json"
cli send --workspace "$WORKSPACE" --surface "$SURFACE" "'$FIXTURE'" >/dev/null
cli send-key --workspace "$WORKSPACE" --surface "$SURFACE" Return >/dev/null
for _ in $(seq 1 150); do [ -f "$RUN_DIR/ready" ] && break; sleep 0.2; done
[ -f "$RUN_DIR/ready" ] || { cli read-screen --workspace "$WORKSPACE" --surface "$SURFACE"; echo 'Host fixture did not execute' >&2; exit 1; }
cli send --workspace "$WORKSPACE" --surface "$SURFACE" 'interactive host input' >/dev/null
cli send-key --workspace "$WORKSPACE" --surface "$SURFACE" Return >/dev/null
for _ in $(seq 1 100); do [ -f "$RUN_DIR/proof" ] && break; sleep 0.1; done
[ -f "$RUN_DIR/proof" ] || { echo 'Host PTY input did not round-trip' >&2; exit 1; }
[ "$(cat "$RUN_DIR/host-colors")" = 256 ]
jq -e --arg workspace "$WORKSPACE" '.caller.workspace_id == $workspace' "$RUN_DIR/host-identify.json" >/dev/null
flatpak "${flatpak_args[@]}" --command=env dev.limux.linux "${private_env[@]}" \
  /app/bin/limux --socket "$SOCKET" --id-format both --json list-workspaces >"$RUN_DIR/sandbox-workspaces.json"
jq -e --arg workspace "$WORKSPACE" '.workspaces | any(.workspace_id == $workspace)' "$RUN_DIR/sandbox-workspaces.json" >/dev/null
flatpak "${flatpak_args[@]}" --command=env dev.limux.linux "${private_env[@]}" \
  /app/bin/limux --socket "$SOCKET" hooks claude install >"$RUN_DIR/hook-install.txt"
HOOK_COMMAND="$(jq -r '.hooks.SessionStart[0].hooks[0].command' "$HOME/.claude/settings.json")"
printf '%s' '{"session_id":"flatpak-hook-session","hook_event_name":"SessionStart"}' \
  | env LIMUX_SOCKET="$SOCKET" LIMUX_WORKSPACE_ID="$WORKSPACE" LIMUX_SURFACE_ID="$SURFACE" \
    /bin/sh -c "$HOOK_COMMAND" >"$RUN_DIR/installed-hook-output.json"
jq -e '.sessions | has("flatpak-hook-session")' "$LIMUX_AGENT_HOOK_STATE_DIR/claude-hook-sessions.json" >/dev/null
cp "$HOME/.claude/settings.json" "$RUN_DIR/installed-hooks.json"
flatpak "${flatpak_args[@]}" --command=env dev.limux.linux "${private_env[@]}" \
  /app/bin/limux --socket "$SOCKET" hooks opencode install >"$RUN_DIR/opencode-install.txt"
HOST_CLI="$(sed -n 's/^const LIMUX_COMMAND = \(.*\);$/\1/p' "$HOME/.config/opencode/plugins/limux-session.js" | jq -r .)"
[ -x "$HOST_CLI" ]
"$HOST_CLI" --socket "$SOCKET" --json identify >"$RUN_DIR/host-launcher-identify.json"
jq -e '.name == "limux-control"' "$RUN_DIR/host-launcher-identify.json" >/dev/null
printf '%s\n' "$HOST_CLI" >"$RUN_DIR/host-cli-path"
cli read-screen --workspace "$WORKSPACE" --surface "$SURFACE" >"$RUN_DIR/screen.txt"
rg -q FLATPAK_HOST_OK "$RUN_DIR/screen.txt"
WINDOW="$(xdotool search --onlyvisible --name '^Limux v' | head -1)"
cli send --workspace "$WORKSPACE" --surface "$SURFACE" "stty size > '$RUN_DIR/size-before'" >/dev/null
cli send-key --workspace "$WORKSPACE" --surface "$SURFACE" Return >/dev/null
for _ in $(seq 1 50); do [ -s "$RUN_DIR/size-before" ] && break; sleep 0.1; done
xdotool windowsize --sync "$WINDOW" 1000 700
sleep 0.3
cli send --workspace "$WORKSPACE" --surface "$SURFACE" "stty size > '$RUN_DIR/size-after'" >/dev/null
cli send-key --workspace "$WORKSPACE" --surface "$SURFACE" Return >/dev/null
for _ in $(seq 1 50); do [ -s "$RUN_DIR/size-after" ] && break; sleep 0.1; done
[ -s "$RUN_DIR/size-before" ] && [ -s "$RUN_DIR/size-after" ]
if cmp -s "$RUN_DIR/size-before" "$RUN_DIR/size-after"; then
  echo 'Host PTY dimensions did not follow the window resize' >&2
  exit 1
fi
if command -v import >/dev/null; then import -window root "$RUN_DIR/flatpak-host-shell.png"; fi
printf 'PASS: installed Flatpak host shell, PTY/cwd/environment, CLI routes, installed hooks, and restored-agent cleanup\nArtifacts: %s\n' "$RUN_DIR"
