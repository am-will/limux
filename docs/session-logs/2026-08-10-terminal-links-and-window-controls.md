# 2026-08-10: Clickable terminal links and window controls

## Delivered

- Terminal links can be opened with `Ctrl` + primary click, even when a
  Ghostty `open_url` key binding is absent from the user's configuration.
- The terminal context menu now offers **Open Link** alongside **Copy URL**.
- Link targets are handed to the system URL handler without a Limux scheme
  allow-list. This supports registered application and local-file links as
  well as web links, matching Ghostty's link-opening behavior. As with
  opening a link manually, users should only activate links they trust.
- Limux always draws its libadwaita header bar and ensures that minimize,
  maximize, and close controls are all present. The pane toolbar's close
  button remains a pane-only action.

## Root cause and resolution

On Wayland, Limux previously hid its header when the compositor advertised
`zxdg_decoration_manager_v1`. Libadwaita had already selected client-side
decorations for the application window, however, so the compositor did not
provide replacement window controls. This was most visible on KDE Plasma/KWin
and left the window with no minimize, maximize, or application-close buttons.

The host now always retains the libadwaita header and completes the GTK
decoration layout while preserving a user's preferred side and ordering when
they already specify all three standard controls.

## Validation

The Linux host passed:

```bash
cargo fmt --check
LD_LIBRARY_PATH=ghostty/zig-out/lib cargo test -p limux-host-linux
cargo clippy -p limux-host-linux -- -D warnings
cargo build --release -p limux-host-linux --bin limux
```

The host test suite completed with 227 passing tests. The full workspace suite
still has the pre-existing `limux-cli` transcript-fallback test failure noted
in `CLAUDE.md`; it is unrelated to this change.
