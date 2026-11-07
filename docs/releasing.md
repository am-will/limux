# Releasing Limux

Releases are cut from a version-bumped, green `main` commit. Publishing a
GitHub release triggers Linux package, RPM, and AUR automation.

## Cut a release

1. Bump `workspace.package.version` in `Cargo.toml` and refresh `Cargo.lock`:

   ```bash
   cargo check -p limux-protocol
   ./scripts/check.sh
   ```

2. Merge the version bump through a pull request and wait for the `main`
   `Rust Quality` run to pass.

3. Publish the release at that exact merge commit:

   ```bash
   version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
   sha=$(gh api repos/am-will/limux/commits/main --jq .sha)
   gh release create "v$version" \
     --repo am-will/limux \
     --target "$sha" \
     --title "Limux v$version" \
     --generate-notes
   ```

4. Wait for `Build Linux Release Packages` and `Build RPM Package`. A
   successful tagged Linux build triggers `Publish to AUR`.

5. Verify release contains tarball, Debian package, AppImage, and RPM. Confirm
   AUR `limux-bin` reports matching version.

## Verify packaged runtime

`./scripts/smoke-release-artifacts.sh <version> all [artifact-directory]`
checks all four exact packages without rebuilding or installing them. It needs
`uv`, Weston, D-Bus, Mesa software rendering, `bwrap`, `xdg-dbus-proxy`, and the
package runtime dependencies. The build host must allow unprivileged user
namespaces. In addition to file/linkage checks, each extracted package runs in
a private Wayland session with isolated HOME, XDG directories, D-Bus, and a control socket.
The check requires a healthy terminal, executed shell input and screen
readback, and an embedded browser that loads a loopback page and sends its
JavaScript callback.

AppImage checks launch its extracted `AppRun` unchanged, including the bundled
WebKit process paths and library environment. The launcher materializes short,
absolute helper paths for bubblewrap and patches a private library copy in an
executable cache. Tarball, Debian, and RPM checks
use their packaged host and Ghostty library with host GTK/WebKit dependencies.
The harness does not add sandbox-bypass overrides or use the desktop session.
It preserves the packaged host's sandbox policy and checks that the WebKit web
process has seccomp filtering, `NoNewPrivs`, no effective capabilities, and a
separate user namespace. A browser load without those restrictions fails.

Ubuntu CI keeps `kernel.apparmor_restrict_unprivileged_userns=1` and uses
`scripts/ci-webkit-sandbox.sh` to load a temporary allowance for the test command.
The wrapper checks real namespace creation and removes its profile on exit.
A separate installed-Debian smoke exercises the package's exact host-path
allowance without that wrapper.

The package runtime harness also accepts `--deny-userns`, which restricts only
the test child with seccomp, and `--expect-browser-unavailable`, which checks an
environment whose policy already denies sandbox startup. Both require usable
terminal input/readback, truthful browser health, rejected browser creation,
and preservation of the saved browser URL, name, and pin across a restart.
`--explicit-disable-sandbox` tests an explicitly requested sandbox bypass and must
not be used as evidence of sandbox isolation.

These checks do not replace cross-distribution testing, particularly the NixOS
graphics environment. Validate Flatpak's actual launcher separately with the
installed development package.

## Rebuild release assets

Both package workflows support manual dispatch. Select release tag as workflow
ref and provide matching version:

```bash
gh workflow run release-linux.yml --ref v0.1.22 -f version=0.1.22
gh workflow run release-rpm.yml --ref v0.1.22 -f version=0.1.22
```

`scripts/validate-release-version.sh` rejects malformed versions and mismatches
between workflow input, package version, and checked-out Cargo workspace.
