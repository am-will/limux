# Limux Agent Hooks

These templates wire supported coding-agent hook systems into Limux session
restore tracking and completion notifications. Default setup covers Codex,
Claude Code, Gemini CLI, and Antigravity (`agy`); OpenCode remains opt-in via
`limux hooks setup opencode`.

The preferred install path is the CLI installer:

```bash
limux hooks setup
```

That writes the equivalent configuration into each agent's user config:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini/settings.json` or `~/.gemini/settings.json` |
| Antigravity (`agy`) | `~/.gemini/config/hooks.json` |

Use the files in this directory as canonical examples when reviewing or
manually repairing an agent config:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`
- `agy-hooks.json`

Each command calls `limux --json hooks <agent> <event>` and is guarded by a
per-agent disable variable:

```bash
LIMUX_CODEX_HOOKS_DISABLED=1
LIMUX_CLAUDE_HOOKS_DISABLED=1
LIMUX_GEMINI_HOOKS_DISABLED=1
LIMUX_AGY_HOOKS_DISABLED=1
```
