# CLAUDE.md — project context for Claude Code

> This file is auto-loaded by Claude Code at the start of a session
> inside the limux repo. It's a short, Claude-oriented companion to
> [`AGENTS.md`](AGENTS.md). For full architectural depth, read
> `AGENTS.md` and `docs/cmux-parity-plan.md`.

## What is this project?

Limux is a GTK4 + libadwaita + libghostty terminal workspace manager for
Linux, ported from manaflow-ai's macOS `cmux`. It exposes a Unix-socket
control API so coding agents (including you) can drive the GUI from a
terminal session inside a limux workspace.

## First-boot checklist (do this before editing)

1. **Confirm the branch and parity status:**

   ```bash
   git branch --show-current
   git log --oneline main..HEAD
   ```

   Active work has been on `feat/cmux-parity`. If you're on `main`,
   switch. Don't rebase shipped commits without asking.

2. **Run the quality gate** before *and* after your changes:

   ```bash
   ./scripts/check.sh
   ```

   This runs `cargo fmt --check`, `cargo clippy --workspace
   --all-targets -- -D warnings`, and `cargo test --workspace`
   (184+ tests). All three must stay green.

3. **Read the living roadmap** in
   [`docs/cmux-parity-plan.md`](docs/cmux-parity-plan.md) before
   starting any agent-integration feature. Phases 1, 3, 4, 5 are done;
   Phase 2 is partial; Phase 6 is deferred. Exact commits are in that
   file and in [`AGENTS.md`](AGENTS.md).

## The two-binary gotcha

- `target/debug/limux` is the **GTK app** from `limux-host-linux`. It
  only understands GTK flags — `limux --help` will show you
  `--help-gapplication`, not `agent-team`.
- `target/debug/limux-cli` is the **CLI** from `limux-cli`. This is
  what implements `agent-team`, `notify`, `claude-hook`, etc.

Always test CLI subcommands with `./target/debug/limux-cli …`, not
plain `limux`. Installed users get `limux` as a symlink to the CLI.

## Build & test cheat sheet

```bash
# Fast iteration (no zig / ghostty rebuild, no release profile)
cargo check --workspace
cargo check -p limux-host-linux           # ~5s GTK/FFI feedback
cargo test -p limux-cli                   # CLI unit tests only

# Full release (requires the vendored ghostty submodule built)
(cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast)
cargo build --release

# Format (run before committing — rustfmt will complain about
# multi-line push((...)) calls otherwise)
cargo fmt
```

## Headless verification (no display required)

For integration-style smoke tests, the GTK host runs under `xvfb-run`:

```bash
sudo pacman -S --needed xorg-server-xvfb     # or apt-get install -y xvfb

xvfb-run -a ./target/release/limux &
./target/release/limux-cli agent-team --agents codex,claude --cwd /tmp/demo
./target/release/limux-cli send --workspace claude '<agent-msg …>'
kill %1
```

`agent-team --dry-run` requires no host at all — it exercises the
launcher map and the `build_agents_md` template path, which the
`agent_team_tests` unit tests already cover.

## Where to make changes — common tasks

| Task | File(s) |
|---|---|
| Add a new agent to `agent-team` | `agent_launch_command` at `rust/limux-cli/src/main.rs:922` |
| Tweak the auto-generated AGENTS.md template | `build_agents_md` at `rust/limux-cli/src/main.rs:1051` |
| Add a new CLI subcommand | dispatch table near `"agent-team" => …` in `rust/limux-cli/src/main.rs:~2203` |
| Route a new method through the GUI bridge | `rust/limux-host-linux/src/control_bridge.rs` (pass `allow_name=true` if agents target peers by workspace name) |
| Full-vocabulary control calls (tests, no GUI) | dispatched through `limux-core::Dispatcher` + `ControlState` |
| Surface / pane state in the UI | `rust/limux-host-linux/src/window.rs` (the single `PaneCallbacks` constructor is around line 3173) |

## Pitfalls — read these, they will save you

- **ID mismatch:** host-linux uses string IDs (`String` for workspace,
  `u32` pane id, uuid `String` tab id); `limux-core` uses `u64`. Build
  `LIMUX_SURFACE_ID` as `format!("{pane_id}:{tab_id}")`. There is no
  `SurfaceId` type in host-linux.
- **`PaneCallbacks` has exactly one constructor.** When you add a
  field, the compiler points you there — don't go hunting for a second
  call site.
- **Ghostty `env_vars` lifetime:** Ghostty `dupeZ`s keys and values into
  its own arena (`ghostty/src/apprt/embedded.zig:573`), so the `Vec<CString>`
  + `Vec<ghostty_env_var_s>` pattern in `terminal.rs::create_terminal`
  only needs to outlive the `ghostty_surface_new` call. No static
  lifetime, no leaks. Re-realize is guarded by the early-return in the
  realize closure.
- **Vendored ghostty is read-only** from the limux perspective. Work
  through the C API in `ghostty/include/ghostty.h`.
- **Commit identity:** `am-will <william@am-will.dev>` (already
  configured on this machine).
- **Clippy is a hard gate.** `-D warnings` — don't disable lints to pass
  CI; fix them.
- **Do not commit** generated artifacts, build outputs, or `target/`.

## The live user-facing CLI surface (what I ship)

```bash
# Fire a libadwaita toast + sidebar unread badge from any agent
limux notify --subtitle "needs review" --body "blocked on auth choice" "Input needed"

# Drop-in hook handlers — translate hook JSON on stdin into notify/send
echo '{"event":"stop"}' | limux claude-hook --event stop

# Multi-agent collaboration team — one workspace per agent, auto-launches
# each CLI, writes AGENTS.md in shared cwd with the <agent-msg> protocol
limux agent-team --agents codex,claude --cwd "$PWD"
```

The inter-agent protocol is **not** defined here. It lives in the
**other** AGENTS.md that `limux agent-team` writes into the user's
shared cwd at runtime. That file is regenerated each time the command
runs — the template source is `build_agents_md` in `limux-cli/src/main.rs`.

## Conventions

- `main` tracks upstream. Topic branches: `feat/cmux-parity`, `fix/…`.
- Don't open PRs or issues from inside Claude Code without asking.
- Keep one source of truth per concept: command metadata, launcher
  maps, workspace IDs, etc.
- Prefer small domain modules; split by domain, not vague helpers.
- Pure logic stays separate from GTK widget wiring where possible.
- Add regression tests when fixing behavior — see the
  `agent_team_tests` module at the bottom of `limux-cli/src/main.rs`
  for the expected shape.

## In case of doubt

- **Roadmap & phase status** → `docs/cmux-parity-plan.md`
- **Contributor / architecture deep-dive** → `AGENTS.md`
- **Maintainability rules** → `docs/maintainability.md`
- **User-facing install/usage** → `README.md`
- **Agent-to-agent message format** → the AGENTS.md written by
  `limux agent-team` into the shared cwd (not this file)
