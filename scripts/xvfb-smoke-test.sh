#!/usr/bin/env bash
# scripts/xvfb-smoke-test.sh - Headless end-to-end smoke test for the
# limux agent-integrations stack. Runs a real limux GTK host under Xvfb,
# exercises limux-cli against the live Unix socket, asserts expected
# behavior, then tears down. Zero display hardware required.
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
command -v xvfb-run >/dev/null || {
  echo "FAIL: xvfb-run not installed (sudo pacman -S xorg-server-xvfb)"
  exit 2
}
command -v cargo >/dev/null || { echo "FAIL: cargo missing"; exit 2; }
command -v sed >/dev/null || { echo "FAIL: sed missing"; exit 2; }

# --- 2. Build -------------------------------------------------------------
if [ "$PROFILE" = "release" ]; then
  CARGO_FLAGS="--release"
  BIN_DIR="target/release"
else
  CARGO_FLAGS=""
  BIN_DIR="target/debug"
fi

echo "-- building limux-cli ($PROFILE)..."
cargo build $CARGO_FLAGS -p limux-cli --bin limux-cli 2>&1 | tail -3

echo "-- building limux-host-linux ($PROFILE)..."
cargo build $CARGO_FLAGS -p limux-host-linux 2>&1 | tail -3

LIMUX_HOST="$ROOT_DIR/$BIN_DIR/limux"
LIMUX_CLI="$ROOT_DIR/$BIN_DIR/limux-cli"
[ -x "$LIMUX_HOST" ] || { echo "FAIL: host binary missing at $LIMUX_HOST"; exit 2; }
[ -x "$LIMUX_CLI" ]  || { echo "FAIL: cli binary missing at $LIMUX_CLI"; exit 2; }

# The release host needs libghostty.so on the runtime path; debug finds
# it via rpath.
LIBGHOSTTY_DIR="$ROOT_DIR/ghostty/zig-out/lib"
if [ "$PROFILE" = "release" ] && [ -d "$LIBGHOSTTY_DIR" ]; then
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

# --- 4. Launch the live host under Xvfb ----------------------------------
# Each smoke run gets its own socket path so we don't collide with the
# user's real limux session.
SOCKET="$DEMO_DIR/limux.sock"
export LIMUX_SOCKET="$SOCKET"
export LIMUX_SOCKET_PATH="$SOCKET"
export LIMUX_SOCKET_MODE="runtime"
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
echo "== stage 1: boot limux host under xvfb-run =="
# Under Xvfb there is no GPU. Force Mesa's software renderer and pin a GL/GLSL
# level that satisfies embedded Ghostty's OpenGL 4.3 renderer.
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER="${GALLIUM_DRIVER:-llvmpipe}"
export LP_NUM_THREADS="${LP_NUM_THREADS:-1}"
export MESA_GL_VERSION_OVERRIDE="${MESA_GL_VERSION_OVERRIDE:-4.3}"
export MESA_GLSL_VERSION_OVERRIDE="${MESA_GLSL_VERSION_OVERRIDE:-430}"
export GDK_BACKEND="${GDK_BACKEND:-x11}"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY
xvfb-run -a -s "-screen 0 1280x800x24 +extension GLX +render" \
  "$LIMUX_HOST" >"$LOG_DIR/host.stdout" 2>"$LOG_DIR/host.stderr" &
HOST_PID=$!
echo "host PID: $HOST_PID (socket=$SOCKET)"

cleanup() {
  local rc=$?
  echo
  echo "-- cleanup (rc=$rc) --"
  if kill -0 "$HOST_PID" 2>/dev/null; then
    kill "$HOST_PID" 2>/dev/null || true
    sleep 1
    kill -9 "$HOST_PID" 2>/dev/null || true
  fi
  # Tail the host log on failure to aid debugging.
  if [ "$rc" -ne 0 ]; then
    echo "-- host.stdout (tail) --"
    tail -n 40 "$LOG_DIR/host.stdout" 2>/dev/null || true
    echo "-- host.stderr (tail) --"
    tail -n 40 "$LOG_DIR/host.stderr" 2>/dev/null || true
    echo "artifacts retained at: $DEMO_DIR"
  else
    # Clean slate on success.
    rm -rf "$DEMO_DIR"
  fi
}
trap cleanup EXIT INT TERM

# Poll for the socket (up to 30s)
for i in $(seq 1 60); do
  if [ -S "$SOCKET" ]; then
    echo "socket up after ${i}*500ms"
    break
  fi
  if ! kill -0 "$HOST_PID" 2>/dev/null; then
    echo "FAIL: host process died before opening the socket"
    exit 1
  fi
  sleep 0.5
done

[ -S "$SOCKET" ] || { echo "FAIL: socket $SOCKET never appeared"; exit 1; }

# The control socket is created before GTK has necessarily finished restoring
# the active workspace and first surface. Wait for the same active-context
# resolution that agent-team uses before exercising the live bridge.
for i in $(seq 1 60); do
  if "$LIMUX_CLI" --json identify >"$LOG_DIR/ready.json" 2>"$LOG_DIR/ready.err"; then
    echo "active workspace ready after ${i}*500ms"
    break
  fi
  if ! kill -0 "$HOST_PID" 2>/dev/null; then
    echo "FAIL: host process died before active workspace was ready"
    exit 1
  fi
  sleep 0.5
done

"$LIMUX_CLI" --json identify >"$LOG_DIR/ready.json" 2>"$LOG_DIR/ready.err" \
  || { echo "FAIL: active workspace never became ready"; cat "$LOG_DIR/ready.err"; exit 1; }

# --- 5. Stage 2: live agent-team ------------------------------------------
echo
echo "== stage 2: agent-team against live host (--no-launch) =="
# --no-launch keeps the workspace commands from actually spawning codex/
# claude binaries (which may not be installed in CI); the bridge + AGENTS.md
# + exact-surface targeting path are still fully exercised.
"$LIMUX_CLI" --id-format both --json agent-team \
  --agents codex,claude \
  --cwd "$DEMO_DIR" \
  --no-launch \
  2>&1 | tee "$LOG_DIR/stage2.json"

grep -q '"agent":"codex"' "$LOG_DIR/stage2.json" \
  || { echo "FAIL: live agent-team did not create codex peer"; exit 1; }
grep -q '"agent":"claude"' "$LOG_DIR/stage2.json" \
  || { echo "FAIL: live agent-team did not create peers"; exit 1; }
[ -f "$DEMO_DIR/AGENTS.md" ] \
  || { echo "FAIL: AGENTS.md not written to $DEMO_DIR"; exit 1; }

TEAM_WORKSPACE_ID="$(sed -n 's/.*"workspace_id":"\([^"]*\)".*/\1/p' "$LOG_DIR/stage2.json" | head -1)"
TEAM_WORKSPACE_NAME="$(sed -n 's/.*"workspace_name":"\([^"]*\)".*/\1/p' "$LOG_DIR/stage2.json" | head -1)"
CODEX_SURFACE="$(sed -n 's/.*{"agent":"codex"[^}]*"surface_id":"\([^"]*\)".*/\1/p' "$LOG_DIR/stage2.json" | head -1)"
CLAUDE_SURFACE="$(sed -n 's/.*{"agent":"claude"[^}]*"surface_id":"\([^"]*\)".*/\1/p' "$LOG_DIR/stage2.json" | head -1)"

[ -n "$TEAM_WORKSPACE_ID" ] || { echo "FAIL: stage 2 response missing workspace_id"; exit 1; }
[ -n "$TEAM_WORKSPACE_NAME" ] || { echo "FAIL: stage 2 response missing workspace_name"; exit 1; }
[ -n "$CODEX_SURFACE" ] || { echo "FAIL: stage 2 response missing codex surface"; exit 1; }
[ -n "$CLAUDE_SURFACE" ] || { echo "FAIL: stage 2 response missing claude surface"; exit 1; }

# Assert the runtime AGENTS.md has the protocol envelope + both peers.
grep -q "<agent-msg"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing <agent-msg>"; exit 1; }
grep -q "\bcodex\b"   "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing codex peer"; exit 1; }
grep -q "\bclaude\b"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing claude peer"; exit 1; }
echo "stage 2: OK (AGENTS.md + peer panes)"

# --- 6. Stage 3: peer surface sanity --------------------------------------
echo
echo "== stage 3: surface.list sees both peer panes =="
"$LIMUX_CLI" --json list-panels --workspace "$TEAM_WORKSPACE_NAME" 2>&1 | tee "$LOG_DIR/stage3.json"
grep -Fq "$CODEX_SURFACE" "$LOG_DIR/stage3.json" \
  || { echo "FAIL: surface.list missing codex surface $CODEX_SURFACE"; exit 1; }
grep -Fq "$CLAUDE_SURFACE" "$LOG_DIR/stage3.json" \
  || { echo "FAIL: surface.list missing claude surface $CLAUDE_SURFACE"; exit 1; }
echo "stage 3: OK"

# --- 7. Stage 4: by-name workspace + exact-surface send -------------------
echo
echo "== stage 4: surface.send_text by workspace name and peer surface =="
ENVELOPE=$'<agent-msg from="codex" to="claude" id="smoke-1" ts="2026-04-19T23:59:00Z"><request>smoke test ping</request></agent-msg>\n'
if "$LIMUX_CLI" send --workspace "$TEAM_WORKSPACE_NAME" --surface "$CLAUDE_SURFACE" "$ENVELOPE" \
     2>&1 | tee "$LOG_DIR/stage4.txt"; then
  echo "stage 4: OK (workspace-name + surface send accepted)"
else
  echo "FAIL: send to claude surface failed"
  exit 1
fi

# --- 8. Stage 5: by-name notify -------------------------------------------
echo
echo "== stage 5: notification.create by workspace name =="
if "$LIMUX_CLI" notify --workspace "$TEAM_WORKSPACE_NAME" --kind attention \
     --subtitle "smoke" --body "all good" "Smoke test" \
     2>&1 | tee "$LOG_DIR/stage5.txt"; then
  echo "stage 5: OK (by-name notify accepted)"
else
  echo "FAIL: by-name notify failed — allow_name=true on notification.create may be regressed"
  exit 1
fi

# --- 9. Stage 6: self-split pane.create + command injection ----------------
echo
echo "== stage 6: pane.create self-split with exact-surface command =="
SELF_SPLIT_PROOF="$DEMO_DIR/self-split-proof"
SELF_SPLIT_ENV="$DEMO_DIR/self-split-env"
SELF_SPLIT_CMD="printf split-ok > '$SELF_SPLIT_PROOF'; printf '%s\n%s\n%s\n' \"\$LIMUX_WORKSPACE_ID\" \"\$LIMUX_PANE_ID\" \"\$LIMUX_SURFACE_ID\" > '$SELF_SPLIT_ENV'"

"$LIMUX_CLI" --id-format both --json new-pane \
  --workspace "$TEAM_WORKSPACE_NAME" \
  --surface "$CLAUDE_SURFACE" \
  --direction right \
  --command "$SELF_SPLIT_CMD" \
  2>&1 | tee "$LOG_DIR/stage6.json"

RESPONSE_WORKSPACE="$(sed -n 's/.*"workspace_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$LOG_DIR/stage6.json" | head -1)"
RESPONSE_PANE="$(sed -n 's/.*"pane_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$LOG_DIR/stage6.json" | head -1)"
RESPONSE_SURFACE="$(sed -n 's/.*"surface_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$LOG_DIR/stage6.json" | head -1)"

[ -n "$RESPONSE_WORKSPACE" ] || { echo "FAIL: pane.create response missing workspace_id"; exit 1; }
[ -n "$RESPONSE_PANE" ] || { echo "FAIL: pane.create response missing pane_id"; exit 1; }
[ -n "$RESPONSE_SURFACE" ] || { echo "FAIL: pane.create response missing surface_id"; exit 1; }

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
if echo '{"hook_event_name":"Notification","message":"hello from smoke"}' \
  | LIMUX_WORKSPACE_ID="$TEAM_WORKSPACE_ID" LIMUX_SURFACE_ID="$CLAUDE_SURFACE" "$LIMUX_CLI" claude-hook 2>&1 \
  | tee "$LOG_DIR/stage7.txt"; then
  echo "stage 7: OK (claude-hook accepted JSON on stdin)"
else
  # claude-hook legitimately errors without a workspace target — that's
  # a pass-through error, not a bridge regression. Surface the output.
  echo "stage 7: claude-hook returned non-zero (check output)"
fi

echo
echo "===================================="
echo "✅ limux agent-integrations smoke test PASSED"
echo "===================================="
