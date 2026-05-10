# Limux

A GPU-accelerated terminal workspace manager for Linux, powered by Ghostty's rendering engine. A special thanks to the cmux contributors who inspired this build. 

If you are on Mac, please visit https://github.com/manaflow-ai/cmux to download the original. 

https://github.com/user-attachments/assets/6f3047c2-e2b6-49f2-b536-570a1570d0f8

## Features

- **GPU-rendered terminals** via embedded Ghostty (OpenGL)
- **Workspaces** with folder-based naming, persistence across restarts, and sidebar management
- **Split panes** (horizontal/vertical) with keyboard navigation
- **Tabbed terminals** within each pane
- **Built-in browser** (WebKitGTK)
- **Right-click context menu** with copy, paste, split, clear
- **Drag-and-drop** workspace reordering with favorites/pinning
- **Animated sidebar** collapse/expand

## Install

Download the latest release from [GitHub Releases](https://github.com/am-will/limux/releases).

**Debian/Ubuntu (.deb)** — recommended:
```bash
sudo dpkg -i ./limux_0.1.14_amd64.deb
```

**AppImage** — portable across Ubuntu 24.04-era desktops and newer, no install needed:
```bash
chmod +x Limux-0.1.14-x86_64.AppImage
./Limux-0.1.14-x86_64.AppImage
```

Release AppImages are built and checked on the Ubuntu 24.04 `GLIBC_2.39`
floor. Limux still uses the host GTK4, libadwaita, and WebKitGTK runtime
libraries, so older distributions may need the `.deb`, tarball, or a source
build with matching system packages instead.

**Tarball** — manual install:
```bash
tar xzf limux-*-linux-x86_64.tar.gz
cd limux-*-linux-x86_64
sudo ./install.sh
```

**Arch Linux (unofficial AUR package)** — community-maintained by [antonbarchukov](https://github.com/antonbarchukov):
```bash
yay -S limux-bin
```

The AUR package is available at [`limux-bin`](https://aur.archlinux.org/packages/limux-bin). Thanks to [antonbarchukov](https://github.com/antonbarchukov) for packaging Limux for Arch users. Arch packaging is not currently maintained by upstream; please report AUR packaging issues to the package maintainer first. See [issue #5](https://github.com/am-will/limux/issues/5).

To uninstall:
```bash
# deb
sudo apt remove limux

# tarball
sudo ./install.sh --uninstall
```

### System dependencies

```bash
# Ubuntu/Debian
sudo apt install libgtk-4-1 libadwaita-1-0 libwebkitgtk-6.0-4
```

## Build from source

### Prerequisites

- Rust toolchain (stable)
- Zig
- GTK4, libadwaita, WebKitGTK dev packages
- Initialized Ghostty submodule

```bash
# Install dev dependencies (Ubuntu/Debian)
sudo apt install libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev pkg-config build-essential

# Initialize the Ghostty submodule and build the embedded library
git submodule update --init --recursive
(cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast)

# Build limux
cargo build --release

# Run (point to libghostty.so location)
LD_LIBRARY_PATH=../ghostty/zig-out/lib:$LD_LIBRARY_PATH ./target/release/limux
```

### Package a release tarball

```bash
./scripts/package.sh
```

This builds the binary, bundles `libghostty.so`, icons, and an install script into a tarball.
`package.sh` also rebuilds `libghostty.so` with `ReleaseFast` and `-Dcpu=baseline`, so Zig and the initialized Ghostty submodule must be present.

## Development

Run the canonical local quality gate before committing:

```bash
./scripts/check.sh
```

Repository maintainability rules live in [`docs/maintainability.md`](docs/maintainability.md).

## Agent integrations

Limux ships first-class hooks for coding agents (Codex, Claude Code, and
Gemini CLI). Every terminal limux spawns auto-exports
`LIMUX_WORKSPACE_ID` / `LIMUX_SURFACE_ID` / `LIMUX_PANE_ID` /
`LIMUX_TAB_ID` / `LIMUX_SOCKET`, so the CLI auto-targets the right place
with no flags needed from inside the agent's own terminal.

```bash
# Fire a libadwaita toast + sidebar unread badge from any agent
limux notify --subtitle "needs review" --body "blocked on auth choice" "Input needed"

# Install Limux session-restore hooks for supported agents
limux hooks setup

# Drop-in hook handlers translate hook JSON on stdin into notify/session state
echo '{"event":"stop"}' | limux claude-hook --event stop
echo '{"event":"finished"}' | limux gemini-hook --event finished

# Spin up a multi-agent collaboration team — one workspace per agent,
# launches each agent's CLI, and writes AGENTS.md describing the
# <agent-msg> XML protocol so peers can talk to each other:
limux agent-team --agents codex,claude --cwd "$PWD"
# → Codex and Claude can now do:
#   limux send --workspace claude $'<agent-msg from="codex" to="claude" id="…" ts="…">…</agent-msg>\n'

# Or split the current agent's pane and launch another terminal agent.
# Inside Limux, workspace/surface/pane default from LIMUX_*:
limux new-pane --direction right --command claude
# Live GTK self-spawn currently supports terminal panes only.

# Explicit source targets are also accepted and serialized unchanged:
limux new-pane --workspace "$LIMUX_WORKSPACE_ID" --surface "$LIMUX_SURFACE_ID" \
  --pane "$LIMUX_PANE_ID" --direction down --command "codex"

# Keep both agents in the same workspace on separate splits/tabs:
limux identify --json
limux list-panels --workspace "$LIMUX_WORKSPACE_ID"
limux send --workspace "$LIMUX_WORKSPACE_ID" --surface "<peer-surface-id>" \
  $'<agent-msg from="codex" to="claude" id="…" ts="…">…</agent-msg>\n'
```

See the auto-generated `AGENTS.md` (written into the shared cwd) for
the full protocol spec, peer table, and editable Policies section.

Checked-in hook templates live in [`hooks/`](hooks/). They mirror
`limux hooks setup` for Codex, Claude Code, and Gemini CLI; OpenCode is
omitted until its hook integration is ready.

Coding agents working on **limux itself** should read [`AGENTS.md`](AGENTS.md)
and [`CLAUDE.md`](CLAUDE.md) in the repo root — those cover the build
loop, crate map, and the `feat/cmux-parity` roadmap tracked in
[`docs/cmux-parity-plan.md`](docs/cmux-parity-plan.md).

## Keyboard shortcuts

Most default shortcuts use `Ctrl`. Fullscreen defaults to `F11`. Custom remaps may also use `Cmd`, which Limux maps to either the Linux `Meta` or `Super` modifier. `Opt` maps to `Alt`.

### App

| Shortcut | Action |
|---|---|
| `Ctrl+Q` | Quit Limux |
| `Ctrl+Alt+N` | Open a new Limux instance |
| `F11` | Toggle fullscreen |

### Browser

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+L` | Open the focused browser page in a new split |
| `Ctrl+L` | Focus browser address bar |
| `Ctrl+[` | Browser back |
| `Ctrl+]` | Browser forward |
| `Ctrl+R` | Browser reload |
| `Ctrl+Alt+I` | Open Web Inspector |
| `Ctrl+Alt+C` | Open Web Inspector (console-only targeting is not exposed by WebKitGTK) |

### Find

| Shortcut | Action |
|---|---|
| `Ctrl+F` | Open find on the focused terminal or browser |
| `Ctrl+G` | Find next |
| `Ctrl+Shift+G` | Find previous |
| `Ctrl+Shift+F` | Hide find |
| `Ctrl+E` | Use selection for find |

### Terminal

| Shortcut | Action |
|---|---|
| `Ctrl+K` | Clear scrollback |
| `Ctrl+Shift+C` | Copy selection |
| `Ctrl+Shift+V` | Paste |
| `Ctrl++` | Increase font size |
| `Ctrl+-` | Decrease font size |
| `Ctrl+Shift+0` | Reset font size |

### Workspace And Pane

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+N` | New workspace (folder picker) |
| `Ctrl+Shift+W` | Close workspace |
| `Ctrl+Shift+Left/Right` | Cycle tabs in focused pane |
| `Ctrl+Shift+D` | Split down |
| `Ctrl+Shift+T` | New terminal tab in the focused pane |
| `Ctrl+D` | Split right |
| `Ctrl+W` | Close focused pane |
| `Ctrl+M` | Toggle sidebar |
| `Ctrl+Shift+M` | Toggle top bar |
| `Ctrl+T` | New terminal tab |
| `Ctrl+Arrow` | Focus pane in direction |
| `Ctrl+PageDown/Up` | Next or previous workspace |
| `Ctrl+1-9` | Switch to workspace by number |

## Architecture

```
rust/
  limux-host-linux/    # GTK4/Adwaita UI (window, sidebar, panes, tabs)
  limux-ghostty-sys/   # FFI bindings to libghostty
  limux-core/          # Command dispatcher and state engine
  limux-protocol/      # Socket wire format types
  limux-control/       # Unix socket server
  limux-cli/           # CLI client
```

The terminal rendering is handled entirely by Ghostty's embedded library (`libghostty.so`), which provides GPU-accelerated OpenGL rendering. The UI layer is native GTK4 with libadwaita.

## License

MIT
