# Flatpak packaging

A Flatpak manifest for Limux (issue #40), building the same artifacts as the
source AUR package (`PKGBUILD-source.template`) but rooted at `/app`.

## Files

- `dev.limux.linux.yml` — the flatpak-builder manifest. App-id `dev.limux.linux`
  matches the existing `dev.limux.linux.desktop` and
  `dev.limux.linux.metainfo.xml`.
- `cargo-sources.json` — offline sources for all 147 crates.io dependencies,
  generated from `Cargo.lock`. Regenerate after any dependency change:

  ```bash
  # from https://github.com/flatpak/flatpak-builder-tools
  python3 cargo/flatpak-cargo-generator.py Cargo.lock -o flatpak/cargo-sources.json
  ```

  The workspace has no git dependencies, so this is fully offline; each entry's
  `sha256` comes straight from `Cargo.lock`.

## Building

```bash
flatpak install -y flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Sdk.Extension.ziglang//25.08
# Until Ghostty's Zig deps are vendored (open item 1), the zig build step needs
# the network, so add: --install-deps-from=flathub is not enough — build the
# `limux` module with a `--share=network` build-arg (or set it in the manifest
# for a local build).
flatpak-builder --user --install --force-clean build-dir flatpak/dev.limux.linux.yml
flatpak run dev.limux.linux
```

## Install layout (`/app`)

| Artifact | Path |
|---|---|
| `target/release/limux-cli` | `/app/bin/limux` |
| `target/release/limux` (GTK host) | `/app/libexec/limux/limux-host` |
| `ghostty/zig-out/lib/libghostty-internal.so` | `/app/lib/limux/libghostty-internal.so` |
| Ghostty resources / terminfo | `/app/share/limux/{ghostty,terminfo}` |
| desktop / metainfo / app icons | `/app/share/{applications,metainfo,icons/hicolor/*/apps}` |
| `-symbolic` pane/browser action icons | `/app/share/icons/hicolor/scalable/actions` |

`limux-host-linux` resolves the Ghostty resource dir relative to its own
executable (`set_ghostty_runtime_env_for_exe` in
`rust/limux-host-linux/src/main.rs`), so no `/usr` paths are hardcoded and the
`/app` tree is discovered at runtime. `RUSTFLAGS` in the manifest overrides the
`/usr/local/lib/limux` rpath from `.cargo/config.toml` with `/app/lib/limux`.

## finish-args rationale

- `--socket=wayland` + `--socket=fallback-x11`, `--share=ipc` — the GTK4 shell.
- `--device=dri` — Ghostty renders terminals with OpenGL.
- `--share=network` — terminals run networked commands and the built-in browser
  (WebKitGTK) needs the network.
- `--talk-name=org.freedesktop.Notifications` — desktop notifications.
- `--filesystem=xdg-run/limux` — exposes the control socket to the host. `--filesystem=host`
  does *not* cover `$XDG_RUNTIME_DIR` (the sandbox replaces it with a private tmpfs), so
  without this the app's `limux.sock` is invisible to a host `limux` CLI. See below.
- `--filesystem=host` — deliberately broad: a terminal that runs arbitrary shell
  commands cannot be meaningfully confined to a subtree. Narrow it if a future
  design sandboxes the shells.

## Host shell (open limitation)

Terminals currently run the **runtime's** `/bin/sh` inside the sandbox, not the
host shell, so host tools (`git`, `cargo`, the project toolchain) are missing from
`PATH` — the reviewer's `PATH=/app/bin:/usr/bin` observation. This is not yet fixed.

Ghostty already implements host-shell spawning: built with `-Dflatpak=true` and
running in a sandbox it spawns the shell through the
`org.freedesktop.Flatpak.Development.HostCommand` portal
(`ghostty/src/os/flatpak.zig`, `FlatpakHostCommand` in `ghostty/src/termio/Exec.zig`),
resolving the host login shell and `HOME` from the host `passwd`. But that flag does
**not compile in Limux's build**: `flatpak.zig` does `@import("gio_c")`, and Ghostty's
build only wires up the `gio_c` module for *exe* steps — the `if (step.kind != .lib)`
guard at `SharedDeps.zig:667` skips it for the `-Dapp-runtime=none` **library** we
build, so the `zig build` step fails with `no module named 'gio_c'`. (Upstream
Ghostty's own Flatpak builds the full GTK exe, where the module is present.)

Two candidate fixes, both bigger than a manifest tweak (see open item 4):

- **Patch Ghostty's build** to also provide `gio_c` (and link `gio-2.0`, not `gtk4`)
  for `lib` steps under `-Dflatpak`. Uses Ghostty's native portal path, but modifies
  the vendored/pinned `ghostty` source and pulls in the `translate_c` build dep
  (worsening the offline-vendoring item).
- **Wrap the shell on the Limux side** in `flatpak-spawn --host` (present in the
  runtime at `/usr/bin/flatpak-spawn`) when running sandboxed. No Ghostty change, but
  Limux must resolve the host login shell and forward env (`TERM`, the `LIMUX_*`
  control vars), cwd, and the PTY itself — partly reimplementing the portal logic.

## Driving the app from a terminal (control socket)

Limux's whole point is that a coding agent in a terminal can drive the GUI over a
Unix control socket. The host binds it at `$XDG_RUNTIME_DIR/limux/limux.sock`
(`resolve_socket_path` / `SocketMode::Runtime`). Under Flatpak, `$XDG_RUNTIME_DIR`
inside the sandbox is a private tmpfs, so the socket is invisible to the host until
`--filesystem=xdg-run/limux` (above) bind-mounts the host's `$XDG_RUNTIME_DIR/limux`
into the sandbox at the same path. With that mount there are two supported CLI routes:

1. **Host-installed `limux`** (from the AUR/source package) resolves the same
   `$XDG_RUNTIME_DIR/limux/limux.sock` and reaches the Flatpak app directly.
2. **In-sandbox CLI** — `flatpak run --command=limux dev.limux.linux <args>` runs
   `/app/bin/limux` against the same socket.

Socket round-trip smoke test (run after `flatpak run dev.limux.linux` is up):

```bash
# From the host, using a host-installed limux:
limux list-workspaces
# Or entirely inside the sandbox, no host install required:
flatpak run --command=limux dev.limux.linux list-workspaces
```

Either should print the running app's workspaces (a JSON/text list, not a connection
error), confirming the host↔app socket path end to end.

## Verification status

Built end to end with `org.flatpak.Builder` 1.4.9 on **GNOME 49** (freedesktop
25.08 SDK; `ziglang` ships Zig 0.16.0 as Ghostty needs, `rust-stable` ships
rustc 1.98) and launched on a Wayland session:

- All sources resolve: the two git sources fetch and `cargo-sources.json`
  vendors all 147 crates offline.
- `disable-submodules: true` on the limux source is required — without it the
  repo's `ghostty` submodule and the pinned `ghostty` git source both claim the
  same directory and the build dies on a `.git` collision.
- **GNOME 49 is the minimum runtime.** The GTK4 crates (`gdk4` 0.11,
  `cairo-rs`/`gdk-pixbuf` 0.22) require rustc ≥ 1.92; GNOME 48's `rust-stable`
  extension only ships 1.89, so the Rust build fails there.
- libghostty (Zig) and the Rust workspace both compile; the app installs and
  runs — `flatpak run dev.limux.linux` maps a GTK window titled
  "Limux v0.1.30" and initialises its control socket.

One caveat on the verifying build: it enabled network for the module
(`--share=network`) so the `zig build` step could fetch Ghostty's Zig packages.
That is not Flathub-compliant — see open item 1.

## Open items before this is Flathub-ready

1. **Offline vendoring of Ghostty's Zig build dependencies.** The Rust side is
   fully vendored via `cargo-sources.json`, but the `zig build` step in the
   `ghostty` submodule still fetches its own Zig packages. Flathub builds run
   with no network, so those packages must be vendored (e.g. prefetched into
   `ZIG_GLOBAL_CACHE_DIR` and shipped as an additional source, or expressed as
   `type: archive` sources). Until then, build locally with a network-enabled
   build (`--share=network` build-arg on the module).
2. **`--filesystem=host` scope** and the app-id/domain (`limux.dev`) that
   metainfo/lint expect are maintainer calls — see the PR description.
3. **metainfo screenshots + release entries.** `flatpak-builder-lint` wants at
   least one `<screenshot>` and a `<releases>` entry in the metainfo before
   Flathub submission; upstream owns that file.
4. **Host-shell execution.** Terminals run the sandbox `/bin/sh`, so host tools
   are off `PATH`. Ghostty's native `-Dflatpak=true` path doesn't compile in the
   `-Dapp-runtime=none` lib build (missing `gio_c` module — see "Host shell"
   above). Needs either a Ghostty build-system patch or a `flatpak-spawn --host`
   wrapper on the Limux side.
