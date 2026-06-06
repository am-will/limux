# Limux Agent Hooks

These templates wire supported coding-agent hook systems into Limux session
restore tracking. Default setup covers Codex, Claude Code, Gemini CLI, and Pi.
OpenCode remains omitted from default setup until its hook path is ready.

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
| Pi | `~/.pi/agent/extensions/limux-hooks.ts` |

Use the files in this directory as canonical examples when reviewing or
manually repairing JSON-based agent configs:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`

Pi uses a generated TypeScript extension instead of a checked-in JSON template.
Regenerate it with:

```bash
limux hooks pi install
```

Each command calls `limux --json hooks <agent> <event>` and is guarded by a
per-agent disable variable:

```bash
LIMUX_CODEX_HOOKS_DISABLED=1
LIMUX_CLAUDE_HOOKS_DISABLED=1
LIMUX_GEMINI_HOOKS_DISABLED=1
LIMUX_PI_HOOKS_DISABLED=1
```
