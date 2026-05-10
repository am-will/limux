# Limux Agent Hooks

These templates wire supported coding-agent hook systems into Limux session
restore tracking. They are intentionally limited to Codex, Claude Code, and
Gemini CLI until the OpenCode hook path is ready.

The preferred install path is the CLI installer:

```bash
limux hooks setup
```

That writes the equivalent configuration into each agent's user config:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Gemini CLI | `~/.gemini/settings.json` |

Use the files in this directory as canonical examples when reviewing or
manually repairing an agent config:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`

Each command calls `limux --json hooks <agent> <event>` and is guarded by a
per-agent disable variable:

```bash
LIMUX_CODEX_HOOKS_DISABLED=1
LIMUX_CLAUDE_HOOKS_DISABLED=1
LIMUX_GEMINI_HOOKS_DISABLED=1
```
