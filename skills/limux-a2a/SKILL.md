---
name: limux-a2a
description: Use inside Limux to identify the current pane/surface/workspace, spawn terminal panes or workspaces, launch agent tasks, coordinate same-workspace surfaces, optionally send callbacks, read peer output, and notify the human.
---

# Limux A2A

Use Limux itself as the live registry. Do not rely on generated files or persistent rosters. Prefer the installed `limux` command; use `./target/.../limux-cli` only when testing this repo build.

## Fast Launch

When asked to launch a peer from inside Limux, split first using the already-exported `LIMUX_*` values. Do not run `identify`, `list-workspaces`, or `read-screen` before the first spawn unless an env value is missing.

```bash
created="$(limux --json new-pane \
  --pane "$LIMUX_PANE_ID" \
  --surface "$LIMUX_SURFACE_ID" \
  --direction right \
  --command 'claude "Task prompt here. Parent surface if you need it: '"$LIMUX_SURFACE_ID"'."')"

child_surface="$(printf '%s\n' "$created" | jq -r '.surface_ref // .surface_id' | sed 's/^surface://')"
child_pane="$(printf '%s\n' "$created" | jq -r '.pane_ref // .pane_id' | sed 's/^pane://')"
```

If the fast launch fails with `failed to connect to socket`, retry that exact `new-pane` command with approved/escalated socket access. If it fails with `Unknown option --json`, switch to the repo-built `./target/debug/limux-cli` or `./target/release/limux-cli` and retry the same command. Only after those retries should you inspect socket/listing state.

## Identity

Each Limux terminal should have:

```bash
printf 'workspace=%s\npane=%s\nsurface=%s\ntab=%s\nsocket=%s\n' \
  "$LIMUX_WORKSPACE_ID" "$LIMUX_PANE_ID" "$LIMUX_SURFACE_ID" "$LIMUX_TAB_ID" "$LIMUX_SOCKET"
```

Fallbacks:

```bash
limux --json identify
limux --json list-workspaces
limux --json list-panels --workspace "$LIMUX_WORKSPACE_ID"
```

Target exact peers with `--surface <surface-id>`; it is found in any workspace, even after its tab moved. Do not add `--workspace` to a surface: it restricts the search to that workspace, and a moved tab's `LIMUX_WORKSPACE_ID` is stale.

## Spawn Panes

Launch tools directly with `new-pane --command`; do not create an empty shell and later inject a long escaped `codex "..."` line. Long injected launch lines can wrap or corrupt the shell input.

```bash
created="$(limux --json new-pane \
  --pane "$LIMUX_PANE_ID" \
  --surface "$LIMUX_SURFACE_ID" \
  --direction right \
  --command 'codex "Task: inspect the diff. Parent surface available if needed: '"$LIMUX_SURFACE_ID"'."')"

child_surface="$(printf '%s\n' "$created" | jq -r '.surface_ref // .surface_id' | sed 's/^surface://')"
child_pane="$(printf '%s\n' "$created" | jq -r '.pane_ref // .pane_id' | sed 's/^pane://')"
```

`new-pane` returns the child workspace, pane, and surface IDs. Capture them immediately. Live GTK pane creation supports terminal panes. For a launch request, this is the first command to run; use identity/listing commands only as fallback diagnostics.

Interactive and non-interactive examples:

```bash
limux --json new-pane --direction right --command 'codex "Task prompt here."'
limux --json new-pane --direction right --command 'codex exec "Task prompt here."'
limux --json new-pane --direction right --command 'claude "Task prompt here."'
limux --json new-pane --direction right --command 'claude -p "Task prompt here."'
```

## Split Layout

For multiple workers in the same workspace, choose the split source explicitly. Do not repeatedly split the parent `right`, and do not repeatedly split the newest tiny pane unless that is intentional.

Column pattern for three workers:

```bash
ws="$LIMUX_WORKSPACE_ID"
parent_pane="$LIMUX_PANE_ID"
parent_surface="$LIMUX_SURFACE_ID"

w1="$(limux --json new-pane --workspace "$ws" --pane "$parent_pane" --surface "$parent_surface" --direction right --command 'codex "Worker 1 task."')"
w1_pane="$(printf '%s\n' "$w1" | jq -r '.pane_ref // .pane_id' | sed 's/^pane://')"
w1_surface="$(printf '%s\n' "$w1" | jq -r '.surface_ref // .surface_id' | sed 's/^surface://')"

w2="$(limux --json new-pane --workspace "$ws" --pane "$w1_pane" --surface "$w1_surface" --direction down --command 'codex "Worker 2 task."')"
w2_pane="$(printf '%s\n' "$w2" | jq -r '.pane_ref // .pane_id' | sed 's/^pane://')"
w2_surface="$(printf '%s\n' "$w2" | jq -r '.surface_ref // .surface_id' | sed 's/^surface://')"

w3="$(limux --json new-pane --workspace "$ws" --pane "$w2_pane" --surface "$w2_surface" --direction down --command 'codex "Worker 3 task."')"
```

Rules:

- First worker: split the parent `right`.
- More workers: split the worker column `down`.
- Keep a worker column around 3 panes on normal screens.
- For more workers or a second column, prefer a new workspace until Limux has a balanced-grid spawn command.

## Workspaces

Use workspaces when a task needs isolation or same-workspace panes would become too small.

```bash
created="$(limux --json new-workspace --cwd "$PWD" --command 'codex "Task prompt here. Parent surface available if needed: '"$LIMUX_SURFACE_ID"'."')"
workspace="$(printf '%s\n' "$created" | jq -r '.workspace_ref // .workspace_id' | sed 's/^workspace://')"
limux --json list-panels --workspace "$workspace"
```

## Observe vs Callback

Default: the parent observes child progress with `read-screen`. This is usually faster and cheaper than making every child report back.

```bash
limux read-screen --surface "$child_surface" --lines 80
limux read-screen --workspace "<workspace>" --surface "<surface>" --lines 80
limux --json surface-health --surface "$child_surface"
```

Give children the parent surface as an available route, not a mandatory report-back requirement:

```text
Parent surface if you need it: <parent-surface>.
Leave concise results visible in your pane. Use limux send to contact the parent only if blocked, finished with something important the parent should not miss, coordinating with siblings, or asked for status pings.
```

Callback command when needed:

```bash
limux send --surface "<parent-surface>" "short message"
limux send-key --surface "<parent-surface>" enter
```

Codex child note: if a callback fails with `failed to connect to socket`, retry the exact `limux send` / `send-key` command with approved or escalated command execution. The surface ID is still valid; socket access is the blocker.

## Send And Coordinate

Send text:

```bash
limux send --surface "<surface>" "hello"
limux send --workspace "<workspace>" --surface "<surface>" "hello"
limux send-key --surface "<surface>" enter
```

Use a small envelope for structured requests:

```bash
limux send --surface "<surface>" $'<limux-msg from-surface="'"$LIMUX_SURFACE_ID"'" to-surface="<surface>" id="'"$(uuidgen)"'" ts="'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'">\n<request>Do the task.</request>\n</limux-msg>\n'
limux send-key --surface "<surface>" enter
```

For related workers, send each child the sibling surface map after all spawns complete:

```bash
limux send --surface "$w1_surface" $'Sibling surfaces: '"$w2_surface $w3_surface"$'\n'
limux send-key --surface "$w1_surface" enter
```

## Human Attention

```bash
limux notify \
  --subtitle "input needed" \
  --body "A pane is blocked and needs a decision" \
  "Limux task needs attention"
```

## Failure Handling

- `failed to connect to socket`: check `LIMUX_SOCKET` and whether the host is running; Codex children may need approved/escalated socket commands.
- `workspace not found`: run `limux --json list-workspaces`.
- `terminal surface not found`: run `limux --json list-panels --workspace ...`; surfaces change when panes/tabs are recreated.
- Text appears but does not run: send `limux send-key ... enter`.
- Target is silent: use `surface-health`, then `read-screen`, then a short follow-up message if needed.
