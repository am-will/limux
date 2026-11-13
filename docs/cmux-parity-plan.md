# cmux-parity plan (revised after architectural discovery)

## Architecture discovery

Limux has **two control servers**:

1. **Standalone `limux-control-server` binary** — uses `limux_core::Dispatcher`
   + `ControlState` and supports the core command vocabulary defined in
   `limux_protocol::methods::CORE_COMMAND_NAMES`. Used for tests and for CLI
   calls when the GUI isn't running.

2. **Embedded bridge inside `limux-host-linux`** — `control_bridge.rs` routes
   methods to the GTK main loop, including `system.{ping,identify,capabilities}`,
   `window.activate`, `workspace.{current,list,create,select,rename,close,set_color,set_group}`,
   `workspace.group_{list,create,rename,set_color,set_collapsed,delete}`,
   `pane.{list,surfaces,create,focus}`,
   `surface.{list,create,close,focus,send_text,send_key,read_text,health}`,
   `tab.action`, `gui_hook.{list,set,clear}`, and `notification.create`.
   Canonical method metadata and aliases are cataloged in
   `rust/limux-protocol/src/methods.rs`.

When the GUI is running, the CLI targets the bridge via the runtime
socket. `list-panes` / `list-panels`, terminal `new-pane --command ...`,
text injection, key-level injection, `surface-health`, and terminal
`read-screen` now work against the running host.

## Delivery strategy (revised)

### Phase 1 — Env auto-wiring ✅ (shipped in 1295d12)

### Phase 2 — Make the bridge a full proxy (🚧 PARTIAL)

Bridge should route unknown methods to a local `Dispatcher` instance
seeded with live GTK state, OR to dedicated per-method `ControlCommand`
variants that interrogate the live state. The cleanest path:

- Maintain a `Arc<Mutex<ControlState>>` owned by the GTK app, kept in
  sync with live workspace/pane/surface state.
- Bridge falls through unknown methods to `Dispatcher::dispatch` on that
  shared state.
- Specific methods that need GTK side-effects (send_text, create_surface,
  notification.create) remain as `ControlCommand` variants.

The terminal introspection path is now bridged directly against live GTK state.
Remaining proxy work is for deferred browser surface commands and broader
dispatcher parity.

**Shipped so far (in 6b8eb1a and follow-up bridge work):**

- `surface.send_text` and `notification.create` now pass `allow_name=true`
  to `parse_optional_workspace_target`, so peers can address each other
  by workspace name (`--workspace claude`) without juggling runtime
  UUIDs. This is what made phase 5 practical.
- `pane.list`, `pane.surfaces`, and `surface.list` now route on the live
  GTK bridge, so agents can discover peer panes/surfaces in a running
  Limux window.
- `surface.send_key` now routes to the exact terminal surface when provided,
  so agents can send deterministic key-level control such as Ctrl-C.
- `surface.health` and `surface.read_text` now route on the live GTK bridge,
  so agents can inspect peer terminal health and visible screen text.
- `pane.create` now routes through the GTK bridge for terminal panes. From
  inside an agent terminal, `limux new-pane --direction right --command claude`
  uses `LIMUX_WORKSPACE_ID`, `LIMUX_SURFACE_ID`, and `LIMUX_PANE_ID` to split
  the caller's pane, create a new terminal, and launch the command there.

**Still open (priority order):**

- Browser command bridge parity.

### Phase 3 — `limux notify` + GUI toast/sidebar integration ✅
`ControlCommand::CreateNotification` wired through the bridge into
`mark_workspace_unread_with_message` + libadwaita toast.
CLI: `limux notify [--workspace <id|name>] [--subtitle <…>] [--body <…>] <title>`.

### Phase 4 — `limux claude-hook` / `opencode-hook` / `gemini-hook` ✅
Reads hook JSON from stdin, translates the agent-specific event vocabulary
into a `notify` (and, where useful, an inline `send`). Drop-in for
`~/.claude/settings.json` hooks blocks.

### Phase 5 — `limux agent-team` + `AGENTS.md` template ✅
`limux agent-team [--agents codex,claude[,opencode,gemini]] [--cwd <path>]
[--no-launch] [--dry-run]`:

- Splits the active workspace into peer terminal panes via `pane.create` and
  launches each requested agent CLI inside the shared `cwd` once `AGENTS.md`
  is published.
- Bridge passes `allow_name=true` to `parse_optional_workspace_target`
  for `surface.send_text` and `notification.create`, and peers can target each
  other directly by surface ID (`limux send --surface <peer-surface-id> …`) or
  workspace name.
- Writes `AGENTS.md` in the shared cwd documenting:
    - the peers table (agent → workspace name → workspace ID → launch cmd),
    - the `<agent-msg from="…" to="…" id="…" reply-to="…" ts="…">` envelope,
    - the exact `limux send` invocation for sending and replying,
    - the `limux notify` escalation path for human input,
    - the `LIMUX_*` env contract every spawned terminal inherits,
    - editable Policies section (timeouts, size limits, destructive-action gating).

### Phase 6 — (deferred) `limux progress`, `limux log`, `limux markdown`
Nice polish, not blockers.

## Why phase 2 first

Without a real bridge, every subsequent feature ends up routing around
the same hole: the GUI owns the ground truth about surfaces/panes but
the CLI can't query it. Fixing this once, properly, makes phases 3–5
small.
