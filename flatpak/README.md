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
| desktop / metainfo / icons | `/app/share/{applications,metainfo,icons}` |

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
- `--filesystem=host` — deliberately broad: a terminal that runs arbitrary shell
  commands cannot be meaningfully confined to a subtree. Narrow it if a future
  design sandboxes the shells.

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
