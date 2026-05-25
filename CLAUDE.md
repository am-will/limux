# CLAUDE.md — project context for Claude Code

Short, Claude-oriented companion to [`AGENTS.md`](AGENTS.md). For
architecture and the full CLI surface, read `AGENTS.md`. For roadmap
status, read [`docs/cmux-parity-plan.md`](docs/cmux-parity-plan.md).

## What is this project?

Limux is a GTK4 + libadwaita + libghostty terminal workspace manager for
Linux, ported from manaflow-ai's macOS `cmux`. It exposes a Unix-socket
control API so coding agents can drive the GUI from a terminal inside a
limux workspace.

## Before editing

Run the quality gate before *and* after your changes:

```bash
./scripts/check.sh   # fmt --check, clippy -D warnings, test --workspace
```

> **Heads-up:** as of this writing one test is failing
> (`cli_arg_tests::hook_session_id_falls_back_to_transcript_stem`,
> assertion at `rust/limux-cli/src/main.rs:3893`). Don't assume a clean
> baseline — run the gate first to see the current state.

## The two-binary gotcha

- `target/debug/limux` — the **GTK app** (`limux-host-linux`). Only
  understands GTK flags. Installed users get this as `limux-host` under
  `libexec`.
- `target/debug/limux-cli` — the **CLI** (`limux-cli`), which implements
  `agent-team`, `notify`, `hooks setup`, `send`, `read-screen`, etc.
  Installed users get this as `limux`.

Run `./target/debug/limux-cli --help` for the full subcommand list —
treat it as the source of truth, not this file.

## Finding code (anchors, not line numbers)

The crates churn, so search by symbol:

```bash
rg -n "fn agent_launch_command|fn build_agents_md" rust/limux-cli/src/main.rs
rg -n "\"agent-team\" =>"                          rust/limux-cli/src/main.rs
rg -n "PaneCallbacks \{"                           rust/limux-host-linux/src/window.rs
```

| Task | Crate / module |
|---|---|
| New agent in `agent-team` | `agent_launch_command` in `rust/limux-cli/src/main.rs` |
| Generated AGENTS.md template | `build_agents_md` in `rust/limux-cli/src/main.rs` |
| New CLI subcommand | dispatch match in `rust/limux-cli/src/main.rs` |
| GUI bridge routing | `rust/limux-host-linux/src/control_bridge.rs` |
| Full-vocabulary control (no GUI) | `limux-core::Dispatcher` + `ControlState` |
| Pane / surface UI state | `rust/limux-host-linux/src/window.rs` (`PaneCallbacks`) |
| Agent-hook installers + templates | `hooks/` + `limux hooks setup` |
| Packaging (AppImage / AUR) | `scripts/package.sh`, `scripts/appimage-webkit.sh`, `PKGBUILD.template` |

## Pitfalls

- **ID mismatch:** host-linux uses `String` workspace ids, `u32` pane id,
  uuid `String` tab id; `limux-core` uses `u64`. Build `LIMUX_SURFACE_ID`
  as `format!("{pane_id}:{tab_id}")`. There is no `SurfaceId` type in
  host-linux.
- **`PaneCallbacks` has one constructor.** Add a field → the compiler
  points you there.
- **Ghostty `env_vars` lifetime:** Ghostty `dupeZ`s keys/values into its
  own arena, so the `Vec<CString>` + `Vec<ghostty_env_var_s>` pattern in
  `terminal.rs::create_terminal` only needs to outlive the
  `ghostty_surface_new` call.
- **Vendored `ghostty/` is read-only.** Work through the C API in
  `ghostty/include/ghostty.h`.
- **AppImage must ship the gdk-pixbuf SVG loader.** Limux's toolbar
  uses `-symbolic` SVG icons; without `libpixbufloader-svg.so` and
  its `librsvg-2.so.2` closure inside the AppImage's `usr/lib/`,
  GTK falls back to the broken-image glyph on hosts that don't have
  the loader installed system-wide (Fedora 44+, minimal containers).
  The bundling logic lives in `scripts/package.sh` and is guarded by
  `assert_pixbuf_svg_loader_bundle`; gate releases on
  `LIMUX_REQUIRE_SVG_LOADER=1` to make a missing loader a hard
  failure.
- **Clippy is a hard gate** (`-D warnings`). Fix lints, don't suppress.
- **Don't commit** `target/` or other build artifacts.

## Conventions

- Topic branches: `fix/issue-NN-…`, `feat/…`. Don't rebase shipped
  commits without asking.
- Don't open PRs or issues from inside Claude Code without asking.
- Keep one source of truth per concept (command metadata, launcher maps,
  workspace IDs).
- Split by domain, not vague helpers. Keep pure logic separate from GTK
  wiring where possible.
- Add regression tests when fixing behavior — see `agent_team_tests` at
  the bottom of `rust/limux-cli/src/main.rs` for the expected shape.

## In case of doubt

- **Architecture / full CLI** → `AGENTS.md`
- **Roadmap & phase status** → `docs/cmux-parity-plan.md`
- **Maintainability rules** → `docs/maintainability.md`
- **User install/usage** → `README.md`
- **Inter-agent message format** → the AGENTS.md that `limux agent-team`
  writes into the shared cwd at runtime (not this repo's AGENTS.md).
