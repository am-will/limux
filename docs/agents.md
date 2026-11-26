# Coding agents

Limux supports Codex, Claude Code, Gemini CLI, Antigravity (`agy`), and OpenCode. Agents can notify you, resume after a restart, and use the window from inside their own terminal.

## Install hooks

```bash
limux hooks setup            # Codex, Claude Code, Gemini CLI, and Antigravity
limux hooks setup agy        # one agent: codex, claude, gemini, agy, or opencode
limux hooks uninstall agy    # remove them again
```

Hooks record each agent session so that Limux can restore it, and they notify you when an agent finishes or needs input. OpenCode is not part of the default setup. Install it with `limux hooks setup opencode`.

The checked-in templates in [`hooks/`](../hooks/) match what `limux hooks setup` writes. [hooks/README.md](../hooks/README.md) lists the files that it changes and how to turn one agent off.

## Save sessions before a restart

Hooks record sessions as agents start or run turns. If agents were already open before you installed the hooks, or you are about to upgrade Limux, save them first:

```bash
limux hooks snapshot --dry-run        # show what would be saved
limux hooks snapshot --save-session   # save it and write session.json
limux save-session                    # only write session.json
```

## Notify

```bash
limux notify --subtitle "needs review" --body "blocked on auth choice" "Input needed"
```

This shows a toast and marks the tab and workspace unread. It works from any script, not only from agents.

Hook handlers translate hook JSON on standard input into notifications and session state:

```bash
echo '{"event":"stop"}' | limux claude-hook --event stop
echo '{"event":"finished"}' | limux gemini-hook --event finished
echo '{"conversationId":"conv-1","terminationReason":"NO_TOOL_CALL","fullyIdle":true}' | limux agy-hook --event stop
```

## Environment

Every terminal in Limux has these variables. The CLI uses them as defaults, so an agent does not need to pass IDs to act on its own terminal.

| Variable | Meaning |
|---|---|
| `LIMUX_WORKSPACE_ID` | The workspace this terminal is in |
| `LIMUX_PANE_ID` | The pane |
| `LIMUX_SURFACE_ID` | The surface (one terminal or browser) |
| `LIMUX_TAB_ID` | The tab |
| `LIMUX_SOCKET` | The control socket |

## Open and drive panes

```bash
# Split the current pane and start another agent
limux new-pane --direction right --command claude

# Name the source pane yourself
limux new-pane --surface "$LIMUX_SURFACE_ID" --pane "$LIMUX_PANE_ID" \
  --direction down --command "codex"

# Find the other panes
limux identify --json
limux list-panels --workspace "$LIMUX_WORKSPACE_ID"

# Type, press a key, read the screen
limux send --surface "<peer-surface-id>" "npm test"
limux send-key --surface "<peer-surface-id>" Enter
limux read-screen --surface "<peer-surface-id>" --lines 40
```

A named surface is found in any workspace. `--workspace` restricts the search to one workspace.

## Agent teams

```bash
limux agent-team --agents codex,claude --cwd "$PWD"
```

This splits the active workspace into one pane per agent, starts each agent, and writes an `AGENTS.md` file in the shared folder. Your pane stays on the left as the orchestrator. The supported agents are `codex`, `claude`, `opencode`, `gemini`, and `agy`.

The generated `AGENTS.md` has the peer table, the `<agent-msg>` message format, and a Policies section that you can edit. Agents then talk to each other:

```bash
limux send --surface "<peer-surface-id>" \
  $'<agent-msg from="codex" to="claude" id="…" ts="…">…</agent-msg>\n'
```

`agent-team` refuses to replace an existing `AGENTS.md`, including one from a previous team, before it creates any panes. Move your existing instructions before you run it, then merge the project policies that you want to keep. To preview the protocol without writing files or contacting the app:

```bash
limux --json agent-team --dry-run   # see agents_md_preview
```

## Agents on a remote host

Agents in an SSH workspace can raise notifications on your local machine. See [Remote agent notifications](ssh.md#remote-agent-notifications-and-helper-installation).
