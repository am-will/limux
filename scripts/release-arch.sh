#!/usr/bin/env bash
# Maps `uname -m` (or the given machine name) to the architecture names used
# in release artifact filenames. Sets ARCH (tarball/AppImage, pacman CARCH),
# DEB_ARCH and RPM_ARCH; fails on unsupported architectures.

resolve_release_arch() {
    local machine="${1:-$(uname -m)}"

    case "$machine" in
        x86_64)
            ARCH="x86_64"
            DEB_ARCH="amd64"
            RPM_ARCH="x86_64"
            ;;
        aarch64)
            ARCH="aarch64"
            DEB_ARCH="arm64"
            RPM_ARCH="aarch64"
            ;;
        *)
            echo "ERROR: unsupported release architecture '${machine}' (supported: x86_64, aarch64)" >&2
            return 1
            ;;
    esac
}
