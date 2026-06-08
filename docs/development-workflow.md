# Development Workflow

This repo has one delivery loop: build the feature in the workspace, verify it
against the right runtime path, then install that same build for local
dogfooding when the change is user-visible.

The checkout pins Rust 1.92 in `rust-toolchain.toml`. Keep that in sync with
the `rust-version` inherited by every crate from the workspace manifest.

## Repository Layout

- `rust/limux-protocol`: shared JSON envelopes and protocol types.
- `rust/limux-core`: in-process command dispatcher and state engine.
- `rust/limux-control`: Unix socket auth, framing, and standalone server.
- `rust/limux-ghostty-sys`: raw Ghostty C API bindings.
- `rust/limux-host-linux`: GTK4/libadwaita host, pane UI, terminal embedding,
  and the production control bridge.
- `rust/limux-cli`: user-facing `limux` CLI and agent integration commands.
- `scripts/`: quality, smoke, local install, and release packaging entrypoints.
- `docs/`: workflow, architecture notes, testing notes, and active plans.

Treat `ghostty/` as vendored input from Limux's point of view. Use Ghostty's C
API rather than editing the vendored tree for Limux features.

## Feature Loop

1. Make the smallest change that fits the crate boundary.
2. Run a narrow check while iterating:

   ```bash
   cargo check -p limux-host-linux
   cargo test -p limux-cli
   cargo check --workspace
   ```

3. Run the canonical gate before handoff or commit:

   ```bash
   ./scripts/check.sh
   ```

4. For live CLI, socket, agent, pane, or notification behavior, exercise the
   production GTK bridge with the smoke harness:

   ```bash
   LIMUX_SMOKE_PROFILE=debug ./scripts/xvfb-smoke-test.sh
   ./scripts/xvfb-smoke-test.sh
   ```

The standalone dispatcher is useful for unit tests, but user-visible CLI
behavior must be checked through the running GTK bridge when possible.

## Local Dogfood Loop

Install the local build that this PC should actually run:

```bash
./scripts/install-local-build.sh
```

By default this installs under `~/.local`. Set `LIMUX_LOCAL_PREFIX` to install
somewhere else, and set `LIMUX_LOCAL_PROFILE=debug` when a debug build is more
useful than release:

```bash
LIMUX_LOCAL_PROFILE=debug ./scripts/install-local-build.sh
```

After installing, restart any already-running Limux GUI. A running process keeps
using the executable and library it mapped at startup.

Confirm the machine is dogfooding the local build:

```bash
./scripts/local-build-status.sh
```

The expected local runtime layout is:

- CLI entrypoint: `$LIMUX_LOCAL_PREFIX/bin/limux` or `~/.local/bin/limux`.
- Host wrapper: `$LIMUX_LOCAL_PREFIX/libexec/limux/limux-host`.
- Host binary: `$LIMUX_LOCAL_PREFIX/libexec/limux/limux-host.bin`.
- Ghostty library: `$LIMUX_LOCAL_PREFIX/lib/limux/libghostty.so`.
- Build info: `$LIMUX_LOCAL_PREFIX/share/limux/local-build.txt`.

If `command -v limux` points at an older package, put the local prefix's `bin`
directory earlier on `PATH` before dogfooding:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Release Loop

Use the release packaging path when a change touches packaging, Ghostty
resources, system linker behavior, distro metadata, or release artifacts:

```bash
./scripts/package.sh
```

CI release workflows build the Linux tarball, deb, AppImage, and RPM on the
Ubuntu 24.04 GLIBC floor. The AUR workflow publishes from release tarballs.

## Handoff Checklist

- Intended files only are changed.
- `./scripts/check.sh` passes, unless an explicit blocker is documented.
- Live GTK bridge behavior is smoke-tested for CLI or runtime changes.
- `./scripts/install-local-build.sh` has been run for user-visible local fixes.
- `./scripts/local-build-status.sh` confirms this PC's `limux` resolves to the
  local build.
- Any already-running Limux GUI has been restarted before claiming dogfood
  coverage.
