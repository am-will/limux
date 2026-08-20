#!/usr/bin/env bash
# scripts/xvfb-smoke-test.sh - Headless end-to-end smoke test for the
# limux agent-integrations stack. Runs a real limux GTK host under Weston,
# exercises limux-cli against the live Unix socket, asserts expected
# behavior, then tears down. Zero display hardware required.
# The historical filename is retained for contributor compatibility.
#
# Usage:
#   ./scripts/xvfb-smoke-test.sh                # release build
#   LIMUX_SMOKE_PROFILE=debug ./scripts/xvfb-smoke-test.sh
set -euo pipefail

PROFILE="${LIMUX_SMOKE_PROFILE:-release}"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

DEMO_DIR="$(mktemp -d -t limux-smoke-XXXXXX)"
LOG_DIR="$DEMO_DIR/logs"
mkdir -p "$LOG_DIR"

echo "== limux agent-integrations smoke test =="
echo "profile:   $PROFILE"
echo "demo dir:  $DEMO_DIR"
echo "log dir:   $LOG_DIR"

# --- 1. Deps --------------------------------------------------------------
command -v weston >/dev/null || {
  echo "FAIL: weston not installed"
  exit 2
}
command -v cargo >/dev/null || { echo "FAIL: cargo missing"; exit 2; }
command -v jq >/dev/null || { echo "FAIL: jq missing"; exit 2; }
command -v sed >/dev/null || { echo "FAIL: sed missing"; exit 2; }
command -v setsid >/dev/null || { echo "FAIL: setsid missing"; exit 2; }

# --- 2. Build -------------------------------------------------------------
if [ "$PROFILE" = "release" ]; then
  CARGO_FLAGS="--release"
  BIN_DIR="target/release"
else
  CARGO_FLAGS=""
  BIN_DIR="target/debug"
fi

echo "-- building limux-cli ($PROFILE)..."
cargo build --locked $CARGO_FLAGS -p limux-cli --bin limux-cli 2>&1 | tail -3

echo "-- building limux-host-linux ($PROFILE)..."
cargo build --locked $CARGO_FLAGS -p limux-host-linux 2>&1 | tail -3

LIMUX_HOST="$ROOT_DIR/$BIN_DIR/limux"
LIMUX_CLI="$ROOT_DIR/$BIN_DIR/limux-cli"
[ -x "$LIMUX_HOST" ] || { echo "FAIL: host binary missing at $LIMUX_HOST"; exit 2; }
[ -x "$LIMUX_CLI" ]  || { echo "FAIL: cli binary missing at $LIMUX_CLI"; exit 2; }

# Use the freshly built Ghostty shared library in every build profile.
LIBGHOSTTY_DIR="$ROOT_DIR/ghostty/zig-out/lib"
if [ -d "$LIBGHOSTTY_DIR" ]; then
  export LD_LIBRARY_PATH="$LIBGHOSTTY_DIR:${LD_LIBRARY_PATH:-}"
fi

# --- 3. Stage 0: dry-run agent-team (no host) ----------------------------
# Fast sanity pass — if this fails nothing else will work.
echo
echo "== stage 0: agent-team --dry-run (no host) =="
"$LIMUX_CLI" agent-team --dry-run \
  --agents codex,claude,opencode,gemini \
  --cwd "$DEMO_DIR" \
  2>&1 | tee "$LOG_DIR/stage0.txt"

grep -q "peers=\[codex, claude, opencode, gemini\]" \
  "$LOG_DIR/stage0.txt" \
  || { echo "FAIL: stage 0 dry-run did not report expected peers"; exit 1; }
echo "stage 0: OK"

# --- 4. Launch the live host under headless Weston -----------------------
# Each smoke run gets its own socket path so we don't collide with the
# user's real limux session.
SOCKET="$DEMO_DIR/limux.sock"
export LIMUX_SOCKET="$SOCKET"
export LIMUX_SOCKET_PATH="$SOCKET"
export LIMUX_SOCKET_MODE="runtime"
unset LIMUX_PANE_ID LIMUX_SURFACE_ID LIMUX_TAB_ID LIMUX_WORKSPACE_ID
export XDG_DATA_HOME="$DEMO_DIR/data"
export XDG_STATE_HOME="$DEMO_DIR/state"
export XDG_RUNTIME_DIR="$DEMO_DIR/runtime"
mkdir -p "$XDG_DATA_HOME/limux" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
cat > "$XDG_DATA_HOME/limux/session.json" <<SMOKE_SESSION
{
  "version": 1,
  "active_workspace_index": 0,
  "top_bar_visible": true,
  "sidebar": { "visible": true, "width": 220 },
  "workspaces": [
    {
      "id": "00000000-0000-4000-8000-000000000001",
      "name": "limux",
      "favorite": false,
      "cwd": "$DEMO_DIR",
      "folder_path": "$DEMO_DIR",
      "autostart_command": "echo native-autostart-visible; if [ -t 0 ] && [ -t 1 ]; then printf native-autostart-pty > $DEMO_DIR/autostart-proof; fi",
      "layout": {
        "kind": "pane",
        "pane_id": 1,
        "active_tab_id": "terminal-0",
        "tabs": [
          {
            "id": "terminal-0",
            "custom_name": null,
            "pinned": false,
            "tab_kind": "terminal",
            "cwd": "$DEMO_DIR"
          }
        ]
      }
    }
  ]
}
SMOKE_SESSION

echo
echo "== stage 1: boot limux host under headless Weston =="
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER="${GALLIUM_DRIVER:-llvmpipe}"
export LP_NUM_THREADS=1
export GDK_BACKEND=wayland
export WAYLAND_DISPLAY=wayland-limux-smoke
HOST_PID=""
WESTON_PID=""

start_compositor() {
  setsid weston \
    --backend=headless-backend.so \
    --socket="$WAYLAND_DISPLAY" \
    --idle-time=0 \
    --width=1280 \
    --height=800 \
    >"$LOG_DIR/weston.log" 2>&1 &
  WESTON_PID=$!

  for _ in $(seq 1 50); do
    if [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]; then
      return 0
    fi
    if ! kill -0 "$WESTON_PID" 2>/dev/null; then
      echo "FAIL: Weston died before opening its Wayland socket"
      return 1
    fi
    sleep 0.1
  done

  echo "FAIL: Weston socket never appeared"
  return 1
}

start_host() {
  local log_name="$1"
  rm -f "$SOCKET"
  setsid "$LIMUX_HOST" >"$LOG_DIR/$log_name.stdout" 2>"$LOG_DIR/$log_name.stderr" &
  HOST_PID=$!
  echo "host process group: $HOST_PID (socket=$SOCKET)"

  for i in $(seq 1 60); do
    if [ -S "$SOCKET" ]; then
      echo "socket up after ${i}*500ms"
      return 0
    fi
    if ! kill -0 "$HOST_PID" 2>/dev/null; then
      echo "FAIL: host process died before opening the socket"
      return 1
    fi
    sleep 0.5
  done

  echo "FAIL: socket $SOCKET never appeared"
  return 1
}

stop_host() {
  local pid="$HOST_PID"
  [ -n "$pid" ] || return 0

  kill -TERM -- "-$pid" 2>/dev/null || true
  for _ in $(seq 1 50); do
    if ! kill -0 -- "-$pid" 2>/dev/null; then
      wait "$pid" 2>/dev/null || true
      HOST_PID=""
      rm -f "$SOCKET"
      return 0
    fi
    sleep 0.1
  done

  echo "FAIL: host process group $pid did not exit after SIGTERM"
  return 1
}

wait_for_healthy_surfaces() {
  local workspace="$1"
  local output="$2"

  for _ in $(seq 1 60); do
    if "$LIMUX_CLI" --json surface-health --workspace "$workspace" >"$output" 2>/dev/null \
      && jq -e '
        .surfaces | length > 0 and all(.[];
          .type == "terminal" and
          .healthy == true and
          .realized == true and
          .process_exited == false and
          (.columns // 0) > 0 and
          (.rows // 0) > 0 and
          (.width_px // 0) > 0 and
          (.height_px // 0) > 0
        )
      ' "$output" >/dev/null; then
      return 0
    fi
    sleep 0.5
  done

  echo "FAIL: terminal surfaces for workspace '$workspace' never became healthy"
  cat "$output" 2>/dev/null || true
  return 1
}

host_child_count() {
  awk '{ count += NF } END { print count + 0 }' /proc/"$HOST_PID"/task/*/children
}

wait_for_host_child_count() {
  local expected="$1"
  for _ in $(seq 1 100); do
    if [ "$(host_child_count)" -eq "$expected" ]; then
      return 0
    fi
    sleep 0.1
  done

  echo "FAIL: host child count is $(host_child_count), expected $expected"
  return 1
}

cleanup() {
  local rc=$?
  echo
  echo "-- cleanup (rc=$rc) --"
  if [ -n "$HOST_PID" ] && kill -0 -- "-$HOST_PID" 2>/dev/null; then
    kill -KILL -- "-$HOST_PID" 2>/dev/null || true
  fi
  if [ -n "$WESTON_PID" ] && kill -0 -- "-$WESTON_PID" 2>/dev/null; then
    kill -KILL -- "-$WESTON_PID" 2>/dev/null || true
  fi
  # Tail the host log on failure to aid debugging.
  if [ "$rc" -ne 0 ]; then
    for log in "$LOG_DIR"/host*.stdout "$LOG_DIR"/host*.stderr "$LOG_DIR"/weston.log; do
      [ -f "$log" ] || continue
      echo "-- $log (tail) --"
      tail -n 40 "$log" || true
    done
    echo "artifacts retained at: $DEMO_DIR"
  else
    # Clean slate on success.
    rm -rf "$DEMO_DIR"
  fi
}
trap cleanup EXIT INT TERM

start_compositor
start_host host

echo
echo "== stage 1b: terminal surface health and screen I/O =="
wait_for_healthy_surfaces limux "$LOG_DIR/stage1-health.json"

for _ in $(seq 1 50); do
  [ -f "$DEMO_DIR/autostart-proof" ] && break
  sleep 0.1
done
[ "$(cat "$DEMO_DIR/autostart-proof" 2>/dev/null)" = "native-autostart-pty" ] \
  || { echo "FAIL: native workspace autostart did not receive a terminal PTY"; exit 1; }
if find "$XDG_RUNTIME_DIR/limux" -maxdepth 1 -name 'workspace-autostart-*.sh' -print -quit \
  | grep -q .; then
  echo "FAIL: workspace autostart script was not removed after launch"
  exit 1
fi
"$LIMUX_CLI" read-screen --workspace limux --scrollback \
  >"$LOG_DIR/stage1-autostart-screen.txt"
[ "$(grep -Fxc "native-autostart-visible" "$LOG_DIR/stage1-autostart-screen.txt")" -eq 1 ] \
  || { echo "FAIL: workspace autostart output was missing or duplicated"; exit 1; }
! grep -Fq "echo native-autostart-visible" "$LOG_DIR/stage1-autostart-screen.txt" \
  || { echo "FAIL: workspace autostart command was visibly typed into the terminal"; exit 1; }
echo "native workspace autostart: OK"

SCREEN_PROOF="limux-screen-proof-$$"
SCREEN_COMMAND="clear; printf '%s\\n' '$SCREEN_PROOF'; printf screen-ok > '$DEMO_DIR/screen-command-proof'"
"$LIMUX_CLI" send --workspace limux "$SCREEN_COMMAND" >"$LOG_DIR/stage1-send.txt"
"$LIMUX_CLI" send-key --workspace limux Enter >"$LOG_DIR/stage1-send-key.txt"
for _ in $(seq 1 50); do
  "$LIMUX_CLI" read-screen --workspace limux >"$LOG_DIR/stage1-screen.txt" 2>/dev/null || true
  if [ -f "$DEMO_DIR/screen-command-proof" ] \
    && grep -Fq "$SCREEN_PROOF" "$LOG_DIR/stage1-screen.txt"; then
    break
  fi
  sleep 0.1
done
[ "$(cat "$DEMO_DIR/screen-command-proof" 2>/dev/null)" = "screen-ok" ] \
  || { echo "FAIL: terminal command did not execute"; exit 1; }
grep -Fq "$SCREEN_PROOF" "$LOG_DIR/stage1-screen.txt" \
  || { echo "FAIL: terminal output was not readable through read-screen"; exit 1; }

SHIFTED_KEY_PROOF="$DEMO_DIR/shifted-key-proof"
"$LIMUX_CLI" send --workspace limux "printf '" >"$LOG_DIR/stage1-shifted-prefix.txt"
"$LIMUX_CLI" send-key --workspace limux at >"$LOG_DIR/stage1-plain-symbol.txt"
"$LIMUX_CLI" send-key --workspace limux '<Shift>a' >"$LOG_DIR/stage1-shifted-letter.txt"
"$LIMUX_CLI" send-key --workspace limux '<Shift>1' >"$LOG_DIR/stage1-shifted-symbol.txt"
"$LIMUX_CLI" send --workspace limux "' > '$SHIFTED_KEY_PROOF'" >"$LOG_DIR/stage1-shifted-suffix.txt"
"$LIMUX_CLI" send-key --workspace limux Enter >"$LOG_DIR/stage1-shifted-enter.txt"
for _ in $(seq 1 50); do
  [ -f "$SHIFTED_KEY_PROOF" ] && break
  sleep 0.1
done
[ "$(cat "$SHIFTED_KEY_PROOF" 2>/dev/null)" = '@A!' ] \
  || { echo "FAIL: plain and shifted send-key did not produce @A!"; exit 1; }
echo "stage 1b: OK (surface realized, terminal I/O and key levels verified)"

echo
echo "== stage 1c: terminal teardown releases workspace processes =="
BASELINE_CHILDREN="$(host_child_count)"
for cycle in $(seq 1 10); do
  "$LIMUX_CLI" --json new-workspace --cwd "$DEMO_DIR" \
    >"$LOG_DIR/stage1c-workspace-$cycle.json"
  TEARDOWN_WORKSPACE="$(jq -r '.workspace_ref' "$LOG_DIR/stage1c-workspace-$cycle.json")"
  "$LIMUX_CLI" select-workspace --workspace "$TEARDOWN_WORKSPACE" \
    >"$LOG_DIR/stage1c-select-$cycle.txt"
  wait_for_host_child_count "$((BASELINE_CHILDREN + 1))"

  "$LIMUX_CLI" --json new-pane --workspace "$TEARDOWN_WORKSPACE" --direction right \
    >"$LOG_DIR/stage1c-pane-$cycle-1.json"
  "$LIMUX_CLI" --json new-pane --workspace "$TEARDOWN_WORKSPACE" --direction down \
    >"$LOG_DIR/stage1c-pane-$cycle-2.json"
  wait_for_host_child_count "$((BASELINE_CHILDREN + 3))"

  EXIT_SURFACE="$(jq -r '.surface_ref' "$LOG_DIR/stage1c-pane-$cycle-2.json")"
  "$LIMUX_CLI" send --workspace "$TEARDOWN_WORKSPACE" --surface "$EXIT_SURFACE" exit \
    >"$LOG_DIR/stage1c-exit-$cycle.txt"
  "$LIMUX_CLI" send-key --workspace "$TEARDOWN_WORKSPACE" --surface "$EXIT_SURFACE" Enter \
    >"$LOG_DIR/stage1c-exit-enter-$cycle.txt"
  wait_for_host_child_count "$((BASELINE_CHILDREN + 2))"

  "$LIMUX_CLI" close-workspace --workspace "$TEARDOWN_WORKSPACE" \
    >"$LOG_DIR/stage1c-close-$cycle.txt"
  wait_for_host_child_count "$BASELINE_CHILDREN"
done
echo "stage 1c: OK (10 multi-pane workspace cycles returned to $BASELINE_CHILDREN child process)"

# --- 5. Stage 2: live agent-team ------------------------------------------
echo
echo "== stage 2: agent-team against live host (--no-launch) =="
# --no-launch keeps the workspace commands from actually spawning codex/
# claude binaries (which may not be installed in CI); pane creation and
# AGENTS.md generation are still fully exercised.
"$LIMUX_CLI" --json --id-format both agent-team \
  --agents codex,claude \
  --cwd "$DEMO_DIR" \
  --no-launch \
  2>&1 | tee "$LOG_DIR/stage2.json"

jq -e '
  .ok == true and
  .workspace_name == "limux" and
  (.peers | map(.agent)) == ["codex", "claude"] and
  all(.peers[];
    (.pane_id | type == "string" and length > 0) and
    (.surface_id | type == "string" and length > 0)
  )
' "$LOG_DIR/stage2.json" >/dev/null \
  || { echo "FAIL: live agent-team returned invalid peer metadata"; exit 1; }
CLAUDE_SURFACE="$(jq -r '.peers[] | select(.agent == "claude") | .surface_id' "$LOG_DIR/stage2.json")"
WORKSPACE_ID="$(jq -r '.workspace_id' "$LOG_DIR/stage2.json")"
[ -f "$DEMO_DIR/AGENTS.md" ] \
  || { echo "FAIL: AGENTS.md not written to $DEMO_DIR"; exit 1; }

# Assert the runtime AGENTS.md has the protocol envelope + both peers.
grep -q "<agent-msg"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing <agent-msg>"; exit 1; }
grep -q "\bcodex\b"   "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing codex peer"; exit 1; }
grep -q "\bclaude\b"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing claude peer"; exit 1; }
echo "stage 2: OK (AGENTS.md + 2 peer panes)"

# --- 6. Stage 3: shared-workspace pane inventory --------------------------
echo
echo "== stage 3: list-panes sees orchestrator and peers =="
"$LIMUX_CLI" --json list-panes --workspace limux >"$LOG_DIR/stage3-panes.json"
jq -e '(.panes | length) == 3 and all(.panes[]; .surface_count == 1)' \
  "$LOG_DIR/stage3-panes.json" >/dev/null \
  || { echo "FAIL: shared workspace does not contain 3 single-surface panes"; exit 1; }
wait_for_healthy_surfaces limux "$LOG_DIR/stage3-health.json"
echo "stage 3: OK (3 healthy terminal panes)"

# --- 7. Stage 4: exact-surface send ---------------------------------------
echo
echo "== stage 4: surface.send_text to peer surface =="
ENVELOPE=$'<agent-msg from="codex" to="claude" id="smoke-1" ts="2026-04-19T23:59:00Z"><request>smoke test ping</request></agent-msg>\n'
if "$LIMUX_CLI" send --workspace limux --surface "$CLAUDE_SURFACE" "$ENVELOPE" \
  2>&1 | tee "$LOG_DIR/stage4.txt"; then
  echo "stage 4: OK (exact-surface send accepted)"
else
  echo "FAIL: exact-surface send to claude peer failed"
  exit 1
fi

# --- 8. Stage 5: surface-targeted notify ----------------------------------
echo
echo "== stage 5: notification.create for peer surface =="
if "$LIMUX_CLI" notify --workspace limux --surface "$CLAUDE_SURFACE" \
     --subtitle "smoke" --body "all good" "Smoke test" \
     2>&1 | tee "$LOG_DIR/stage5.txt"; then
  echo "stage 5: OK (surface-targeted notify accepted)"
else
  echo "FAIL: surface-targeted notify failed"
  exit 1
fi

# --- 9. Stage 6: self-split pane.create + command injection ----------------
echo
echo "== stage 6: pane.create self-split with exact-surface command =="
SELF_SPLIT_PROOF="$DEMO_DIR/self-split-proof"
SELF_SPLIT_ENV="$DEMO_DIR/self-split-env"
SELF_SPLIT_CMD="printf split-ok > '$SELF_SPLIT_PROOF'; printf '%s\n%s\n%s\n' \"\$LIMUX_WORKSPACE_ID\" \"\$LIMUX_PANE_ID\" \"\$LIMUX_SURFACE_ID\" > '$SELF_SPLIT_ENV'"

"$LIMUX_CLI" --json --id-format both new-pane \
  --workspace limux \
  --surface "$CLAUDE_SURFACE" \
  --direction right \
  --command "$SELF_SPLIT_CMD" \
  2>&1 | tee "$LOG_DIR/stage6.json"

jq -e '
  .ok == true and
  .surface_type == "terminal" and
  (.workspace_id | type == "string" and length > 0) and
  (.pane_id | type == "string" and length > 0) and
  (.surface_id | type == "string" and length > 0)
' "$LOG_DIR/stage6.json" >/dev/null \
  || { echo "FAIL: pane.create returned an invalid response"; exit 1; }

RESPONSE_WORKSPACE="$(jq -r '.workspace_id' "$LOG_DIR/stage6.json")"
RESPONSE_PANE="$(jq -r '.pane_id' "$LOG_DIR/stage6.json")"
RESPONSE_SURFACE="$(jq -r '.surface_id' "$LOG_DIR/stage6.json")"

for _ in $(seq 1 50); do
  if [ -f "$SELF_SPLIT_PROOF" ] && [ -f "$SELF_SPLIT_ENV" ]; then
    break
  fi
  sleep 0.1
done

[ -f "$SELF_SPLIT_PROOF" ] || { echo "FAIL: self-split command proof file missing"; exit 1; }
[ "$(cat "$SELF_SPLIT_PROOF")" = "split-ok" ] || { echo "FAIL: self-split proof file has unexpected content"; exit 1; }
[ -f "$SELF_SPLIT_ENV" ] || { echo "FAIL: self-split env file missing"; exit 1; }

ENV_WORKSPACE="$(sed -n '1p' "$SELF_SPLIT_ENV")"
ENV_PANE="$(sed -n '2p' "$SELF_SPLIT_ENV")"
ENV_SURFACE="$(sed -n '3p' "$SELF_SPLIT_ENV")"

[ "$ENV_WORKSPACE" = "$RESPONSE_WORKSPACE" ] || {
  echo "FAIL: spawned pane LIMUX_WORKSPACE_ID ($ENV_WORKSPACE) did not match response ($RESPONSE_WORKSPACE)"
  exit 1
}
[ "$ENV_PANE" = "$RESPONSE_PANE" ] || {
  echo "FAIL: spawned pane LIMUX_PANE_ID ($ENV_PANE) did not match response ($RESPONSE_PANE)"
  exit 1
}
[ "$ENV_SURFACE" = "$RESPONSE_SURFACE" ] || {
  echo "FAIL: spawned pane LIMUX_SURFACE_ID ($ENV_SURFACE) did not match response ($RESPONSE_SURFACE)"
  exit 1
}
echo "stage 6: OK (self-split command ran with fresh LIMUX_* env)"

# --- 10. Stage 7: hook translators end-to-end -----------------------------
echo
echo "== stage 7: claude-hook event translation =="
echo '{"hook_event_name":"Notification","message":"hello from smoke"}' \
  | LIMUX_WORKSPACE_ID="" "$LIMUX_CLI" claude-hook 2>&1 \
  | tee "$LOG_DIR/stage7.txt"
echo "stage 7: OK (claude-hook accepted JSON on stdin)"

# --- 11. Stage 8: session persistence and clean process restart -----------
echo
echo "== stage 8: session persistence and host restart =="
RESTORED_WORKSPACE="limux-restored"
"$LIMUX_CLI" rename-workspace --workspace "$WORKSPACE_ID" "$RESTORED_WORKSPACE"
for _ in $(seq 1 50); do
  if jq -e --arg name "$RESTORED_WORKSPACE" \
    '.workspaces | any(.name == $name)' \
    "$XDG_DATA_HOME/limux/session.json" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
jq -e --arg name "$RESTORED_WORKSPACE" \
  '.workspaces | any(.name == $name)' \
  "$XDG_DATA_HOME/limux/session.json" >/dev/null \
  || { echo "FAIL: renamed workspace was not persisted"; exit 1; }

stop_host
start_host host-restart
"$LIMUX_CLI" list-workspaces >"$LOG_DIR/stage8-workspaces.txt"
grep -Fq "$RESTORED_WORKSPACE" "$LOG_DIR/stage8-workspaces.txt" \
  || { echo "FAIL: renamed workspace was not restored"; exit 1; }
for _ in $(seq 1 50); do
  "$LIMUX_CLI" --json list-panes --workspace "$RESTORED_WORKSPACE" \
    >"$LOG_DIR/stage8-panes.json"
  if jq -e '(.panes | length) == 4' "$LOG_DIR/stage8-panes.json" >/dev/null; then
    break
  fi
  sleep 0.1
done
jq -e '(.panes | length) == 4' "$LOG_DIR/stage8-panes.json" >/dev/null \
  || { echo "FAIL: restored workspace did not retain all panes"; exit 1; }
wait_for_healthy_surfaces "$RESTORED_WORKSPACE" "$LOG_DIR/stage8-health.json"
stop_host
kill -TERM -- "-$WESTON_PID"
for _ in $(seq 1 50); do
  if ! kill -0 -- "-$WESTON_PID" 2>/dev/null; then
    wait "$WESTON_PID" 2>/dev/null || true
    WESTON_PID=""
    break
  fi
  sleep 0.1
done
[ -z "$WESTON_PID" ] || { echo "FAIL: Weston process group did not exit"; exit 1; }
echo "stage 8: OK (session restored and process groups exited)"

echo
echo "===================================="
echo "✅ limux agent-integrations smoke test PASSED"
echo "===================================="
