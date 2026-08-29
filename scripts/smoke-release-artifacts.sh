#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-}"
MODE="${2:-all}"
DIST_DIR="${3:-$ROOT_DIR/dist}"

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version> [linux|rpm|all] [dist-dir]" >&2
    exit 1
fi
"$ROOT_DIR/scripts/validate-release-version.sh" "$VERSION" >/dev/null

case "$MODE" in
    linux | rpm | all) ;;
    *)
        echo "ERROR: mode must be linux, rpm, or all: $MODE" >&2
        exit 1
        ;;
esac

if [ ! -d "$DIST_DIR" ]; then
    echo "ERROR: artifact directory does not exist: $DIST_DIR" >&2
    exit 1
fi
DIST_DIR="$(cd "$DIST_DIR" && pwd)"

ARCH="$(uname -m)"
DEB_ARCH="amd64"
[ "$ARCH" = "aarch64" ] && DEB_ARCH="arm64"
RPM_ARCH="x86_64"
[ "$ARCH" = "aarch64" ] && RPM_ARCH="aarch64"
EXPECTED_VERSION="limux $VERSION"
SMOKE_ROOT="$(mktemp -d -t limux-release-smoke-XXXXXX)"
trap 'find "$SMOKE_ROOT" -depth -delete' EXIT

require_artifact() {
    local path="$1"

    if [ ! -f "$path" ]; then
        echo "ERROR: release artifact is missing: $path" >&2
        exit 1
    fi
}

verify_cli() {
    local cli="$1"
    local label="$2"
    local actual
    local help

    if [ ! -x "$cli" ]; then
        echo "ERROR: $label CLI is missing or not executable: $cli" >&2
        exit 1
    fi
    actual="$("$cli" --version)"
    if [ "$actual" != "$EXPECTED_VERSION" ]; then
        echo "ERROR: $label reports '$actual', expected '$EXPECTED_VERSION'" >&2
        exit 1
    fi
    help="$("$cli" --help 2>&1)"
    if ! grep -q "limux CLI" <<< "$help"; then
        echo "ERROR: $label does not contain the Limux CLI entrypoint" >&2
        exit 1
    fi
}

verify_tree() {
    local prefix="$1"
    local cli="$2"
    local library_dir="$3"
    local label="$4"
    local host="$prefix/libexec/limux/limux-host"
    local ghostty="$library_dir/libghostty-internal.so"
    local dynamic

    verify_cli "$cli" "$label"
    if [ ! -x "$host" ]; then
        echo "ERROR: $label host is missing or not executable: $host" >&2
        exit 1
    fi
    if [ ! -f "$ghostty" ]; then
        echo "ERROR: $label Ghostty library is missing: $ghostty" >&2
        exit 1
    fi
    if [ -e "$prefix/libexec/limux/limux" ]; then
        echo "ERROR: $label contains the legacy host entrypoint" >&2
        exit 1
    fi
    if ! dynamic="$(LD_LIBRARY_PATH="$library_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ldd -r "$host" 2>&1)"; then
        printf '%s\n' "$dynamic" >&2
        exit 1
    fi
    if printf '%s\n' "$dynamic" | grep -Eq "not found|undefined symbol"; then
        printf '%s\n' "$dynamic" >&2
        exit 1
    fi
}

smoke_linux() {
    local tarball="$DIST_DIR/limux-$VERSION-linux-$ARCH.tar.gz"
    local deb="$DIST_DIR/limux_${VERSION}_${DEB_ARCH}.deb"
    local appimage="$DIST_DIR/Limux-${VERSION}-${ARCH}.AppImage"
    local tar_root="$SMOKE_ROOT/tar/limux-$VERSION-linux-$ARCH"
    local deb_root="$SMOKE_ROOT/deb"
    local actual
    local help

    require_artifact "$tarball"
    require_artifact "$deb"
    require_artifact "$appimage"

    mkdir -p "$SMOKE_ROOT/tar" "$deb_root"
    tar -xzf "$tarball" -C "$SMOKE_ROOT/tar"
    verify_tree "$tar_root" "$tar_root/limux" "$tar_root/lib" "tarball"

    dpkg-deb -x "$deb" "$deb_root"
    actual="$(dpkg-deb -f "$deb" Version)"
    if [ "$actual" != "$VERSION" ]; then
        echo "ERROR: Debian package metadata reports '$actual', expected '$VERSION'" >&2
        exit 1
    fi
    verify_tree "$deb_root/usr" "$deb_root/usr/bin/limux" "$deb_root/usr/lib/limux" "Debian package"

    if [ ! -x "$appimage" ]; then
        echo "ERROR: AppImage is not executable: $appimage" >&2
        exit 1
    fi
    actual="$(APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --version)"
    if [ "$actual" != "$EXPECTED_VERSION" ]; then
        echo "ERROR: AppImage reports '$actual', expected '$EXPECTED_VERSION'" >&2
        exit 1
    fi
    help="$(APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --help 2>&1)"
    if ! grep -q "limux CLI" <<< "$help"; then
        echo "ERROR: AppImage does not contain the Limux CLI entrypoint" >&2
        exit 1
    fi

    echo "Linux release artifact smoke: OK"
}

smoke_rpm() {
    local rpm="$DIST_DIR/limux-${VERSION}-1.${RPM_ARCH}.rpm"
    local rpm_root="$SMOKE_ROOT/rpm"
    local metadata_version

    require_artifact "$rpm"
    if ! command -v rpm >/dev/null 2>&1 || ! command -v rpm2cpio >/dev/null 2>&1 || ! command -v cpio >/dev/null 2>&1; then
        echo "ERROR: rpm, rpm2cpio, and cpio are required for RPM smoke checks" >&2
        exit 1
    fi
    metadata_version="$(rpm -qp --qf '%{VERSION}' "$rpm")"
    if [ "$metadata_version" != "$VERSION" ]; then
        echo "ERROR: RPM metadata reports '$metadata_version', expected '$VERSION'" >&2
        exit 1
    fi

    mkdir -p "$rpm_root"
    (
        cd "$rpm_root"
        rpm2cpio "$rpm" | cpio -idm --quiet
    )
    verify_tree "$rpm_root/usr" "$rpm_root/usr/bin/limux" "$rpm_root/usr/lib/limux" "RPM package"

    echo "RPM release artifact smoke: OK"
}

case "$MODE" in
    linux) smoke_linux ;;
    rpm) smoke_rpm ;;
    all)
        smoke_linux
        smoke_rpm
        ;;
esac
