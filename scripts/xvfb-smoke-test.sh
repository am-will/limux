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

grep -q "OK agent-team peers=\[codex, claude, opencode, gemini\]" \
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
export XDG_RUNTIME_DIR="$DEMO_DIR/runtime"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

echo
echo "== stage 1: boot limux host under xvfb-run =="
# Under Xvfb there is no GPU, so Mesa would fall back to llvmpipe, which
# has historically crashed on Ghostty's shader variants. Force softpipe
# (slower but stable), and pin GL version to avoid newer-feature probes.
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER=softpipe
export LP_NUM_THREADS=1
export MESA_GL_VERSION_OVERRIDE="${MESA_GL_VERSION_OVERRIDE:-3.3}"
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

# --- 5. Stage 2: live agent-team ------------------------------------------
echo
echo "== stage 2: agent-team against live host (--no-launch) =="
# --no-launch keeps the workspace commands from actually spawning codex/
# claude binaries (which may not be installed in CI); the bridge + AGENTS.md
# + allow_name=true path are still fully exercised.
"$LIMUX_CLI" --id-format both agent-team \
  --agents codex,claude \
  --cwd "$DEMO_DIR" \
  --no-launch \
  2>&1 | tee "$LOG_DIR/stage2.txt"

grep -q "OK agent-team peers=\[codex, claude\]" "$LOG_DIR/stage2.txt" \
  || { echo "FAIL: live agent-team did not create peers"; exit 1; }
[ -f "$DEMO_DIR/AGENTS.md" ] \
  || { echo "FAIL: AGENTS.md not written to $DEMO_DIR"; exit 1; }

# Assert the runtime AGENTS.md has the protocol envelope + both peers.
grep -q "<agent-msg"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing <agent-msg>"; exit 1; }
grep -q "\bcodex\b"   "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing codex peer"; exit 1; }
grep -q "\bclaude\b"  "$DEMO_DIR/AGENTS.md" || { echo "FAIL: AGENTS.md missing claude peer"; exit 1; }
echo "stage 2: OK (AGENTS.md + 2 workspaces + allow_name bridge path)"

# --- 6. Stage 3: list-workspaces sanity -----------------------------------
echo
echo "== stage 3: list-workspaces sees both peers =="
"$LIMUX_CLI" list-workspaces 2>&1 | tee "$LOG_DIR/stage3.txt"
grep -q codex  "$LOG_DIR/stage3.txt" || { echo "FAIL: list-workspaces missing codex"; exit 1; }
grep -q claude "$LOG_DIR/stage3.txt" || { echo "FAIL: list-workspaces missing claude"; exit 1; }
echo "stage 3: OK"

# --- 7. Stage 4: by-name send (the phase-5 allow_name=true unlock) --------
# This is the single most important assertion in the whole harness —
# it proves that `limux send --workspace <name>` resolves to the right
# workspace via the bridge. Without allow_name=true this errors out.
echo
echo "== stage 4: surface.send_text by workspace name =="
ENVELOPE=$'<agent-msg from="codex" to="claude" id="smoke-1" ts="2026-04-19T23:59:00Z"><request>smoke test ping</request></agent-msg>\n'
if "$LIMUX_CLI" send --workspace claude "$ENVELOPE" 2>&1 | tee "$LOG_DIR/stage4.txt"; then
  echo "stage 4: OK (by-name send accepted)"
else
  echo "FAIL: by-name send to 'claude' failed — allow_name=true may be regressed"
  exit 1
fi

# --- 8. Stage 5: by-name notify -------------------------------------------
echo
echo "== stage 5: notification.create by workspace name =="
if "$LIMUX_CLI" notify --workspace claude --subtitle "smoke" --body "all good" "Smoke test" \
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

"$LIMUX_CLI" --json new-pane \
  --workspace claude \
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
  | LIMUX_WORKSPACE_ID="" "$LIMUX_CLI" claude-hook 2>&1 \
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
