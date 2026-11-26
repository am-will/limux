# Install Limux

Download packages from the [latest release](https://github.com/am-will/limux/releases/latest). Each release has a `SHA256SUMS` file.

## Requirements

Release packages need glibc 2.39 or newer, GTK4, libadwaita, and WebKitGTK 6.0. Ubuntu 24.04, Debian 13, Fedora 40, and Arch Linux meet these requirements, as do later releases. All package formats contain the same binary, so a different format does not lower the glibc requirement. On an older system, build from source on that system.

Releases after v0.1.36 include x86_64 and ARM64 builds. v0.1.36 and earlier are x86_64 only. ARM64 files are named `aarch64`, except the `.deb`, which uses `arm64`.

## Debian and Ubuntu

```bash
sudo apt install ./limux_*_amd64.deb   # x86_64
sudo apt install ./limux_*_arm64.deb   # ARM64
```

To remove it, run `sudo apt remove limux`.

## Fedora

```bash
sudo dnf install ./limux-*.x86_64.rpm   # or ./limux-*.aarch64.rpm
```

To remove it, run `sudo dnf remove limux`.

## Arch Linux

```bash
yay -S limux-bin   # prebuilt release package
yay -S limux       # builds Limux and Ghostty from source
```

This repository publishes both [`limux-bin`](https://aur.archlinux.org/packages/limux-bin) and the source-built [`limux`](https://aur.archlinux.org/packages/limux) package, for `x86_64` and `aarch64` (Arch Linux ARM). The source package is currently behind the releases ([#195](https://github.com/am-will/limux/issues/195)), so prefer `limux-bin`. [Anton Barchukov](https://github.com/antonbarchukov) created the original Arch packaging in [#50](https://github.com/am-will/limux/pull/50).

## AppImage

```bash
chmod +x Limux-*-x86_64.AppImage   # or Limux-*-aarch64.AppImage
./Limux-*-x86_64.AppImage
```

The AppImage bundles Limux, Ghostty resources, the WebKitGTK helper processes, and loader modules such as the gdk-pixbuf SVG loader. It uses these parts of your system:

- The GTK4 and libadwaita runtime.
- `bwrap` (`bubblewrap`) and `xdg-dbus-proxy`. WebKitGTK needs them to sandbox browser processes. Install them from your distribution if they are missing.
- GBM and DRI drivers. The launcher finds the driver directories, so the bundle also works with Mesa on Fedora and Arch family systems.

The embedded [AppImage type-2 runtime](https://github.com/AppImage/type2-runtime) includes FUSE 3 statically. You do not need `libfuse.so.2` or `libfuse.so.3`. It works with versioned FUSE 3 helpers such as `fusermount3`, and with `fusermount`.

The launcher creates private WebKit helper paths under `/tmp` and keeps their executables in `${XDG_CACHE_HOME:-$HOME/.cache}/limux`. The cache directory must allow execution. `/tmp` may be mounted with `noexec`. Limux removes both on exit.

The AppImage library paths apply only to the Limux process. Terminals inside Limux get your original library and loader environment back, so host tools such as Flatpak do not load AppImage libraries.

## Tarball

```bash
tar xzf limux-*-linux-x86_64.tar.gz   # or limux-*-linux-aarch64.tar.gz
cd limux-*-linux-x86_64
sudo ./install.sh
```

To install without root into your home folder:

```bash
./install.sh --prefix="$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
```

To uninstall, run the same command with `--uninstall`, for example `./install.sh --prefix="$HOME/.local" --uninstall`.

## Flatpak

There is a manifest for local builds. It is not on Flathub yet. See [flatpak/README.md](../flatpak/README.md).

## Runtime packages

```bash
# Ubuntu or Debian
sudo apt install libgtk-4-1 libadwaita-1-0 libwebkitgtk-6.0-4

# WebKit sandbox helpers for the AppImage
sudo apt install bubblewrap xdg-dbus-proxy
```

### Clipboard on Wayland

Also install `wl-clipboard`:

```bash
sudo apt install wl-clipboard    # Debian, Ubuntu
sudo dnf install wl-clipboard    # Fedora
sudo pacman -S wl-clipboard      # Arch Linux
```

`Ctrl+Shift+V` is the Limux paste. Plain `Ctrl+V` goes to the program in the terminal, and some programs read the clipboard themselves (fish `Ctrl+V`, for example). Without `wl-paste`, they fall back to X11 tools such as `xclip`. Depending on the compositor, the X11 clipboard may not have what you copied in Limux, and `Ctrl+V` pastes something older than `Ctrl+Shift+V`.

## Browser sandbox

Limux turns on the WebKit sandbox by default. Before the first browser view opens, Limux starts WebKit in a disposable process to check that the sandbox works. If it does not, browser tabs show an explanation and keep their saved state. Terminals stay available. The check is normally quick. A failed start can take fifteen seconds, plus up to two seconds to clean up helpers.

### Ubuntu 24.04 and later

These releases need an [AppArmor allowance for user namespaces](https://documentation.ubuntu.com/release-notes/24.04/#unprivileged-user-namespace-restrictions).

- The Debian and RPM packages include an allowance for the installed app.
- A tarball install into `/usr` or `/usr/local` registers its path when AppArmor is active.
- User and custom-prefix installs print instructions for an administrator.
- A path with special characters needs a profile that you write by hand.

For a source build, an administrator can register the exact executable:

```bash
sudo ./scripts/install-apparmor-profile.sh --install "$(realpath target/debug/limux)"
```

For an AppImage, pass its absolute path instead. Native packages include the same script at `share/limux/install-apparmor-profile.sh`. If you move the executable, run `--remove` with the old path and `--install` with the new one. Restart Limux after you change the policy.

### Running without the sandbox

If `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` is set in the environment, browser isolation is off. Limux never sets it for you, and `limux surface-health` reports `sandboxed: false` while it is in use.
