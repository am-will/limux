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
flatpak install -y flathub org.gnome.Platform//48 org.gnome.Sdk//48 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08 \
  org.freedesktop.Sdk.Extension.ziglang//24.08
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
- `--talk-name=org.freedesktop.portal.*` — WebKitGTK's sandboxed helper
  processes reach the host through the XDG portals.
- `--filesystem=host` — deliberately broad: a terminal that runs arbitrary shell
  commands cannot be meaningfully confined to a subtree. Narrow it if a future
  design sandboxes the shells.

## Verification status

Run through `org.flatpak.Builder` 1.4.9 (GNOME 48 / freedesktop 24.08 SDK,
`rust-stable` and `ziglang` 24.08 extensions — the latter ships Zig 0.16.0,
which is what Ghostty requires):

- All sources resolve: the two git sources fetch and `cargo-sources.json`
  vendors all 147 crates offline.
- `disable-submodules: true` on the limux source is required — without it the
  repo's `ghostty` submodule and the pinned `ghostty` git source both claim the
  same directory and the build dies on a `.git` collision. (Fixed here.)
- The build then reaches the `zig build` step for libghostty.

Not yet completed end to end: the full compile and GUI launch were not finished
in the environment used (the disk filled before the Rust link stage). The
remaining gaps are below.

## Open items before this is Flathub-ready

1. **Offline vendoring of Ghostty's Zig build dependencies.** The Rust side is
   fully vendored via `cargo-sources.json`, but the `zig build` step in the
   `ghostty` submodule still fetches its own Zig packages. Flathub builds run
   with no network, so those packages must be vendored (e.g. prefetched into
   `ZIG_GLOBAL_CACHE_DIR` and shipped as an additional source, or expressed as
   `type: archive` sources). Until then, build locally with a network-enabled
   build (`--share=network` build-arg on the module).
2. **Finish an end-to-end build + GUI launch.** Structure, source resolution,
   the submodule fix, and crate checksums are validated; a full compile and a
   `flatpak run dev.limux.linux` on a Wayland/X11 session still need to confirm
   the runtime resource resolution and WebKitGTK portal wiring.
