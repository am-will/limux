# Flatpak packaging

This manifest builds and installs the current Limux checkout. It is intended for
local development and package validation. The remaining Flathub work is listed
below.

## Build and install

Install `flatpak-builder` and a Flathub remote, then install the build tools:

```bash
flatpak install --user flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Sdk.Extension.golang//25.08
flatpak-builder --user --install --force-clean \
  "$HOME/.cache/limux-flatpak-build" flatpak/dev.limux.linux.yml
flatpak run dev.limux.linux
```

Keep the build directory and builder state under `$HOME`. The nested build
sandbox has its own `/tmp`. The manifest copies this checkout, excluding Git
metadata, `target/`, and the local Ghostty checkout. It fetches the exact Ghostty
revision pinned by Limux separately. This avoids submodule collisions and ensures
that changes under review are included in the package.

GNOME 49 supplies GTK4, libadwaita, and WebKitGTK. The Rust SDK extension must
support the Rust version required by the GTK bindings. Zig is downloaded from
ziglang.org at the exact version required by `scripts/build-ghostty.sh`, with
architecture-specific checksums. The rolling `ziglang` SDK extension can advance
to an incompatible version.

The Rust build is `--locked --offline`. `cargo-sources.json` supplies crates and
checksums from `Cargo.lock`; regenerate it after dependency changes using the
[Flatpak Cargo generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo).
Ghostty's Zig dependencies still require network access, enabled explicitly for
the Limux module. This limitation prevents a Flathub submission today.

## Host terminals

The package builds [host-spawn 1.6.2](https://github.com/1player/host-spawn/tree/v1.6.2)
from source, with a separately pinned archive of its vendored Go dependencies.
Both downloads have SHA-256 checksums in the manifest. No Go dependencies are
fetched during the build.

Inside this Flatpak, Limux uses Ghostty's structured argv C API to launch
`host-spawn`. It creates a PTY, proxies input/output and terminal resize events,
and invokes the Flatpak `HostCommand` interface. The default terminal starts the
host session's `$SHELL` as a login shell, falling back to `/bin/sh`. The terminal
starts in the workspace's project directory. It inherits the host toolchain's
`PATH`; the sandbox's `PATH` is not forwarded.

`COLORTERM`, `LIMUX_*`, and the per-terminal environment are forwarded. Host
terminals use `TERM=xterm-256color`, so applications can use the host terminfo
database without a native Ghostty installation. The bundled `xterm-ghostty`
terminfo entry remains inside the Flatpak.

Configured `command` and `initial-command` values also execute on the host.
`initial-command` applies only to the first successfully created surface.
Restored startup commands override those settings. Shell commands are passed as
one argument to the host `/bin/sh -c`, preserving quotes, variable references,
and newlines. `direct:` keeps Ghostty's literal space splitting and performs no
shell expansion. Native packages retain their existing launch path.

Host shell startup files are responsible for their own shell integration; the
sandbox's Ghostty resources and helper paths are not host executable paths.
Without host-side shell integration, `cd` works in the current terminal, but new
tabs retain the workspace's initial directory. Automatic shell integration through
the host-spawn wrapper remains follow-up work.

## Permissions and install layout

The desktop app uses Wayland with an X11 fallback, devices for terminal rendering,
network access for terminals and the browser, and notifications. These additional
permissions support host terminals and CLI automation:

- `--talk-name=org.freedesktop.Flatpak` permits host command execution. Terminal
  commands run with the user's host permissions.
- `--device=all` shares the host `/dev/pts` namespace as well as rendering devices.
  DRI alone leaves the PTY private to the sandbox: host `isatty` succeeds, but
  `tty` cannot resolve the device and reports `not a tty`. This follows the
  [upstream Ghostty Flatpak approach](https://github.com/ghostty-org/ghostty/discussions/3201).
- `--filesystem=host` exposes project files to the GUI.
- `--filesystem=xdg-run/limux:create` shares the default control socket directory.
  `--filesystem=host` alone does not expose `$XDG_RUNTIME_DIR`.

The CLI is installed at `/app/bin/limux`, the GUI host at
`/app/libexec/limux/limux-host`, and libghostty at
`/app/lib/limux/libghostty-internal.so`. Resources and terminfo are installed under
`/app/share/limux`. The manifest also installs the desktop file, metainfo, app
icons, pane action icons, and licenses. Build-only Zig files are removed before
packaging.

A host-installed Limux CLI can access the default socket directly. Without a
native CLI installation, use the packaged CLI from the host shell:

```bash
flatpak run --command=limux dev.limux.linux list-workspaces
flatpak run --command=limux dev.limux.linux identify
```

Agent hook installation and restored-agent cleanup use a packaged host launcher,
not `/app/bin/limux` or a native CLI on `PATH`. Limux resolves the host deployment
path from `/.flatpak-info` and uses the branch's `active` symlink so hooks survive
package updates. The launcher runs the packaged CLI through host `flatpak run`,
preserving the branch, architecture, arguments, and hook input.

## Runtime validation

After installing the package, build a native test CLI and run the isolated smoke
harness:

```bash
cargo build -p limux-cli --bin limux-cli
./scripts/tests/test-flatpak-runtime.sh
```

The harness requires Xvfb, xauth, xdotool, D-Bus, jq, Git, and a host `tput`. It uses a
private HOME, display, XDG directories, control socket, and D-Bus with only the Flatpak
session helper available. Set `FLATPAK_USER_DIR` to test a separate user
installation, and `LIMUX_TEST_CLI` to use a CLI outside `target/debug`.
`LIMUX_FLATPAK_TEST_SHELL`, `LIMUX_FLATPAK_TEST_COMMAND`, and
`LIMUX_FLATPAK_TEST_INTEGRATION` can select additional shell/configuration cases.

It verifies a host-only fixture, interactive PTY input/output, project `pwd`,
`tty`, host terminfo, forwarded `LIMUX_*` identity, a native CLI request from the
terminal, and a request through the packaged CLI. It retains logs and a screenshot
when ImageMagick is available. The test uses an explicit private socket path to
avoid the user's running session.
It also restores a synthetic agent, verifies automatic cleanup, installs hooks
into the private home, and executes their host launcher with a native `limux`
masked from `PATH`.

## Remaining Flathub work

1. Vendor Ghostty's Zig dependencies and remove the module's network build
   permission. Rust and host-spawn dependencies are already supplied offline.
2. Add metainfo screenshots and release entries, and resolve app-id/domain lint
   requirements before submitting to Flathub.
3. Replace the local checkout source with an immutable upstream release source
   when maintaining a separate Flathub manifest.
