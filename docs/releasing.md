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
`uv`, Weston, D-Bus, Mesa software rendering, and the package runtime dependencies.
In addition to file/linkage checks, each extracted package runs in a private
Wayland session with isolated XDG directories and a control socket. The check
requires a healthy terminal, executed shell input and screen readback, and an
embedded browser that loads a loopback page and sends its JavaScript callback.

AppImage checks launch its extracted `AppRun` unchanged, including the bundled
WebKit process paths and library environment. Tarball, Debian, and RPM checks
use their packaged host and Ghostty library with host GTK/WebKit dependencies.
The harness does not add sandbox-bypass overrides or use the desktop session.
It preserves the packaged host's sandbox policy, which currently disables the
WebKit sandbox by default. A passing smoke does not prove sandbox-enabled
operation. These checks run on the Ubuntu build host; they do not replace
cross-distribution testing, particularly the NixOS graphics environment.

## Rebuild release assets

Both package workflows support manual dispatch. Select release tag as workflow
ref and provide matching version:

```bash
gh workflow run release-linux.yml --ref v0.1.22 -f version=0.1.22
gh workflow run release-rpm.yml --ref v0.1.22 -f version=0.1.22
```

`scripts/validate-release-version.sh` rejects malformed versions and mismatches
between workflow input, package version, and checked-out Cargo workspace.
