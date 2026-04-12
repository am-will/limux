#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Read version from workspace Cargo.toml (single source of truth)
VERSION="${1:-$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')}"
ARCH="$(uname -m)"
DEB_ARCH="amd64"
[ "$ARCH" = "aarch64" ] && DEB_ARCH="arm64"
RPM_ARCH="x86_64"
[ "$ARCH" = "aarch64" ] && RPM_ARCH="aarch64"

PKG_BASE="limux-${VERSION}-linux-${ARCH}"
STAGE="/tmp/limux-staging"
GHOSTTY_INSTALL_ROOT="/tmp/limux-ghostty-install"
GHOSTTY_SO="${ROOT_DIR}/ghostty/zig-out/lib/libghostty.so"
MAX_GLIBC_VERSION="${LIMUX_MAX_GLIBC:-2.39}"
GHOSTTY_SHARE_DIR=""
GHOSTTY_TERMINFO_DIR=""
ICONS_DIR="${ROOT_DIR}/rust/limux-host-linux/icons"
APP_ICONS_DIR="${ROOT_DIR}/rust/limux-host-linux/icons/app"
DESKTOP_FILE="${ROOT_DIR}/rust/limux-host-linux/dev.limux.linux.desktop"
METADATA_FILE="${ROOT_DIR}/rust/limux-host-linux/dev.limux.linux.metainfo.xml"
OUT_DIR="${ROOT_DIR}/dist"

remove_tree() {
    local path="$1"

    if [ ! -e "$path" ]; then
        return 0
    fi

    find "$path" -depth -mindepth 1 ! -type d -exec rm -f {} +
    find "$path" -depth -mindepth 1 -type d -exec rmdir {} + 2>/dev/null || true
    rmdir "$path" 2>/dev/null || true
}

version_gt() {
    local left="$1"
    local right="$2"
    [ "$left" != "$right" ] && [ "$(printf '%s\n%s\n' "$left" "$right" | sort -V | tail -n1)" = "$left" ]
}

glibc_requirement_for() {
    local path="$1"

    if ! command -v objdump >/dev/null 2>&1; then
        return 0
    fi

    objdump -T "$path" 2>/dev/null \
        | grep -oE 'GLIBC_[0-9]+\.[0-9]+' \
        | sed 's/^GLIBC_//' \
        | sort -Vu \
        | tail -n1
}

assert_glibc_compatibility() {
    local path="$1"
    local label="$2"
    local required_glibc

    required_glibc="$(glibc_requirement_for "$path")"
    if [ -z "$required_glibc" ]; then
        echo "WARNING: unable to determine GLIBC requirement for ${label}"
        return 0
    fi

    if version_gt "$required_glibc" "$MAX_GLIBC_VERSION"; then
        echo "ERROR: ${label} requires GLIBC_${required_glibc}, which exceeds the supported release baseline GLIBC_${MAX_GLIBC_VERSION}."
        echo "Build release artifacts inside Ubuntu 24.04 or another environment pinned to GLIBC_${MAX_GLIBC_VERSION} or older."
        echo "Override the baseline intentionally with LIMUX_MAX_GLIBC=<version> if you are targeting a newer distro on purpose."
        exit 1
    fi

    echo "Verified ${label} GLIBC requirement: GLIBC_${required_glibc} (target max GLIBC_${MAX_GLIBC_VERSION})"
}

resolve_ghostty_share_dir() {
    local candidate

    for candidate in \
        "${GHOSTTY_INSTALL_ROOT}/usr/share/ghostty" \
        "${ROOT_DIR}/ghostty/zig-out/share/ghostty" \
        "/usr/local/share/ghostty" \
        "/usr/share/ghostty"
    do
        if [ -d "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

resolve_ghostty_terminfo_dir() {
    local candidate
    local parent

    parent="$(dirname "$GHOSTTY_SHARE_DIR")"

    for candidate in \
        "${GHOSTTY_INSTALL_ROOT}/usr/share/terminfo" \
        "${parent}/terminfo" \
        "/usr/local/share/terminfo" \
        "/usr/share/terminfo"
    do
        if [ -f "${candidate}/g/ghostty" ] || [ -f "${candidate}/x/xterm-ghostty" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

copy_ghostty_terminfo_entries() {
    local source_dir="$1"
    local dest_dir="$2"

    mkdir -p "${dest_dir}/g" "${dest_dir}/x"

    if [ -f "${source_dir}/g/ghostty" ]; then
        cp "${source_dir}/g/ghostty" "${dest_dir}/g/ghostty"
    fi

    if [ -f "${source_dir}/x/xterm-ghostty" ]; then
        cp "${source_dir}/x/xterm-ghostty" "${dest_dir}/x/xterm-ghostty"
    fi
}

build_ghostty_resources() {
    echo "Staging Ghostty resources..."
    remove_tree "$GHOSTTY_INSTALL_ROOT"
    mkdir -p "$GHOSTTY_INSTALL_ROOT"

    (
        cd "${ROOT_DIR}/ghostty"
        DESTDIR="$GHOSTTY_INSTALL_ROOT" \
            zig build \
            --prefix /usr \
            -Doptimize=ReleaseFast \
            -Dcpu=baseline \
            -Demit-docs=false
    )
}

echo "=== Limux Packager ==="
echo "Version: ${VERSION}"
echo "Arch:    ${ARCH}"
echo "GLIBC:   <= ${MAX_GLIBC_VERSION}"

if ! command -v zig >/dev/null 2>&1; then
    echo "ERROR: zig not found in PATH."
    echo "Install Zig, then rerun ./scripts/package.sh"
    exit 1
fi

if [ ! -f "${ROOT_DIR}/ghostty/build.zig" ]; then
    echo "ERROR: Ghostty submodule is missing or uninitialized at ${ROOT_DIR}/ghostty"
    echo "Run: git submodule update --init --recursive"
    exit 1
fi

# Always build libghostty with ReleaseFast to guarantee optimized output.
# Pinning cpu=baseline keeps the shipped library portable across x86_64 CPUs
# that do not expose the builder's ISA extensions, such as AVX-512.
echo "Building libghostty (ReleaseFast, cpu=baseline)..."
(cd "${ROOT_DIR}/ghostty" && zig build -Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline)
build_ghostty_resources

if [ ! -f "$GHOSTTY_SO" ]; then
    echo "ERROR: libghostty.so not found at ${GHOSTTY_SO} after build"
    exit 1
fi

if ! GHOSTTY_SHARE_DIR="$(resolve_ghostty_share_dir)"; then
    echo "ERROR: Ghostty resources directory not found."
    echo "Looked for:"
    echo "  ${ROOT_DIR}/ghostty/zig-out/share/ghostty"
    echo "  /usr/local/share/ghostty"
    echo "  /usr/share/ghostty"
    exit 1
fi

if ! GHOSTTY_TERMINFO_DIR="$(resolve_ghostty_terminfo_dir)"; then
    echo "ERROR: Ghostty terminfo directory not found."
    echo "Looked for:"
    echo "  $(dirname "$GHOSTTY_SHARE_DIR")/terminfo"
    echo "  /usr/local/share/terminfo"
    echo "  /usr/share/terminfo"
    exit 1
fi

# Build release binary
echo "Building release binary..."
cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

BINARY="${ROOT_DIR}/target/release/limux"
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at ${BINARY}"
    exit 1
fi

assert_glibc_compatibility "$GHOSTTY_SO" "libghostty.so"
assert_glibc_compatibility "$BINARY" "limux"

# Clean staging and output
remove_tree "$STAGE"
remove_tree "$OUT_DIR"
mkdir -p "$OUT_DIR"

# =========================================================================
# Helper: populate a prefix tree at a given root
# =========================================================================
populate_tree() {
    local dest="$1"
    local prefix="${2:-/usr/local}"
    local strip_files="${3:-true}"
    local bindir="$dest${prefix}/bin"
    local libdir="$dest${prefix}/lib/limux"
    local ghostty_datadir="$dest${prefix}/share/limux"
    local ghostty_resdir="$ghostty_datadir/ghostty"
    local appdir="$dest${prefix}/share/applications"
    local metadatadir="$dest${prefix}/share/metainfo"
    local icondir="$dest${prefix}/share/icons/hicolor"

    mkdir -p "$bindir" "$libdir" "$ghostty_resdir" "$appdir" "$metadatadir" "$icondir/scalable/actions"

    # Binary
    cp "$BINARY" "$bindir/limux"
    if [ "$strip_files" = "true" ]; then
        strip "$bindir/limux"
    fi
    chmod 755 "$bindir/limux"

    # Shared library
    cp "$GHOSTTY_SO" "$libdir/libghostty.so"
    if [ "$strip_files" = "true" ]; then
        strip --strip-debug "$libdir/libghostty.so"
    fi

    # Ghostty resources required for named themes and shell integration
    cp -r "$GHOSTTY_SHARE_DIR"/. "$ghostty_resdir"
    copy_ghostty_terminfo_entries "$GHOSTTY_TERMINFO_DIR" "$ghostty_datadir/terminfo"

    # Desktop file
    cp "$DESKTOP_FILE" "$appdir/dev.limux.linux.desktop"
    cp "$METADATA_FILE" "$metadatadir/dev.limux.linux.metainfo.xml"

    # Action icons
    if [ -d "$ICONS_DIR/hicolor" ]; then
        cp -r "$ICONS_DIR/hicolor/scalable" "$icondir/" 2>/dev/null || true
    fi
    for svg in "$ICONS_DIR"/*.svg; do
        [ -f "$svg" ] && cp "$svg" "$icondir/scalable/actions/"
    done

    # App launcher icons
    if [ -d "$APP_ICONS_DIR" ]; then
        for size in 16 32 128 256 512; do
            src="${APP_ICONS_DIR}/${size}.png"
            if [ -f "$src" ]; then
                mkdir -p "$icondir/${size}x${size}/apps"
                cp "$src" "$icondir/${size}x${size}/apps/limux.png"
            fi
        done
    fi
}

build_rpm_source_tree() {
    local dest="$1"

    remove_tree "$dest"
    mkdir -p "$dest"
    populate_tree "$dest" "/usr" "false"

    mkdir -p "$dest/etc/ld.so.conf.d"
    echo "/usr/lib/limux" > "$dest/etc/ld.so.conf.d/limux.conf"
}

build_rpm_package() {
    local rpm_src_dir="/tmp/limux-$VERSION"
    local rpm_tarball="/tmp/limux-$VERSION.tar.gz"
    local rpmbuild_dir="/tmp/rpmbuild-$VERSION"
    local rpm_output="$rpmbuild_dir/RPMS/${RPM_ARCH}/limux-${VERSION}-1.${RPM_ARCH}.rpm"

    if ! command -v rpmbuild >/dev/null 2>&1; then
        echo "  WARNING: rpmbuild not found, skipping RPM"
        return 0
    fi

    build_rpm_source_tree "$rpm_src_dir"
    tar -czf "$rpm_tarball" -C /tmp "limux-$VERSION"
    remove_tree "$rpm_src_dir"

    remove_tree "$rpmbuild_dir"
    mkdir -p "$rpmbuild_dir"/{BUILD,RPMS,SOURCES,SPECS}
    cp "$rpm_tarball" "$rpmbuild_dir/SOURCES/"
    cp "$ROOT_DIR/scripts/limux.spec" "$rpmbuild_dir/SPECS/"

    rpmbuild -bb \
        --define "_topdir $rpmbuild_dir" \
        --define "version $VERSION" \
        --target "$RPM_ARCH" \
        "$rpmbuild_dir/SPECS/limux.spec" 2>&1

    if [ -f "$rpm_output" ]; then
        cp "$rpm_output" "$OUT_DIR/"
        echo "  -> dist/limux-${VERSION}-1.${RPM_ARCH}.rpm"
    else
        echo "  WARNING: rpmbuild did not produce expected RPM file"
    fi

    remove_tree "$rpmbuild_dir"
}

# =========================================================================
# 1. Tarball
# =========================================================================
echo ""
echo "--- Building tarball ---"
TARBALL_STAGE="/tmp/${PKG_BASE}"
remove_tree "$TARBALL_STAGE"
mkdir -p "$TARBALL_STAGE"/{lib,share/limux/ghostty,share/limux/terminfo,share/applications,share/icons/hicolor/scalable/actions}
mkdir -p "$TARBALL_STAGE/share/metainfo"

cp "$BINARY" "$TARBALL_STAGE/limux"
strip "$TARBALL_STAGE/limux"
chmod 755 "$TARBALL_STAGE/limux"
cp "$GHOSTTY_SO" "$TARBALL_STAGE/lib/libghostty.so"
strip --strip-debug "$TARBALL_STAGE/lib/libghostty.so"
cp -r "$GHOSTTY_SHARE_DIR"/. "$TARBALL_STAGE/share/limux/ghostty"
copy_ghostty_terminfo_entries "$GHOSTTY_TERMINFO_DIR" "$TARBALL_STAGE/share/limux/terminfo"
cp "$DESKTOP_FILE" "$TARBALL_STAGE/share/applications/dev.limux.linux.desktop"
cp "$METADATA_FILE" "$TARBALL_STAGE/share/metainfo/dev.limux.linux.metainfo.xml"

if [ -d "$ICONS_DIR/hicolor" ]; then
    cp -r "$ICONS_DIR/hicolor/scalable" "$TARBALL_STAGE/share/icons/hicolor/" 2>/dev/null || true
fi
for svg in "$ICONS_DIR"/*.svg; do
    [ -f "$svg" ] && cp "$svg" "$TARBALL_STAGE/share/icons/hicolor/scalable/actions/"
done
if [ -d "$APP_ICONS_DIR" ]; then
    for size in 16 32 128 256 512; do
        src="${APP_ICONS_DIR}/${size}.png"
        if [ -f "$src" ]; then
            mkdir -p "$TARBALL_STAGE/share/icons/hicolor/${size}x${size}/apps"
            cp "$src" "$TARBALL_STAGE/share/icons/hicolor/${size}x${size}/apps/limux.png"
        fi
    done
fi

# Generate install.sh
cat > "$TARBALL_STAGE/install.sh" << 'INSTALL_EOF'
#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr/local"
UNINSTALL=false

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#*=}" ;;
        --uninstall) UNINSTALL=true ;;
        -h|--help)
            echo "Usage: install.sh [--prefix=/usr/local] [--uninstall]"
            exit 0
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

need_root() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "This operation requires root. Re-running with sudo..."
        exec sudo "$0" "$@"
    fi
}

remove_tree() {
    local path="$1"

    if [ ! -e "$path" ]; then
        return 0
    fi

    find "$path" -depth -mindepth 1 ! -type d -exec rm -f {} +
    find "$path" -depth -mindepth 1 -type d -exec rmdir {} + 2>/dev/null || true
    rmdir "$path" 2>/dev/null || true
}

if $UNINSTALL; then
    need_root "$@"
    echo "Uninstalling Limux..."
    rm -f "$PREFIX/bin/limux"
    remove_tree "$PREFIX/lib/limux"
    remove_tree "$PREFIX/share/limux"
    rm -f /etc/ld.so.conf.d/limux.conf
    ldconfig 2>/dev/null || true
    rm -f "$PREFIX/share/applications/limux.desktop"
    rm -f "$PREFIX/share/applications/dev.limux.linux.desktop"
    rm -f "$PREFIX/share/metainfo/dev.limux.linux.metainfo.xml"
    for size in 16 32 128 256 512; do
        rm -f "$PREFIX/share/icons/hicolor/${size}x${size}/apps/limux.png"
    done
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/limux-globe-symbolic.svg"
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/limux-split-horizontal-symbolic.svg"
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/limux-split-vertical-symbolic.svg"
    gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
    appstreamcli refresh-cache --force 2>/dev/null || true
    echo "Limux uninstalled."
    exit 0
fi

need_root "$@"
echo "Installing Limux to ${PREFIX}..."

install -Dm755 "$SCRIPT_DIR/limux" "$PREFIX/bin/limux"
install -Dm644 "$SCRIPT_DIR/lib/libghostty.so" "$PREFIX/lib/limux/libghostty.so"
if [ -d "$SCRIPT_DIR/share/limux" ]; then
    cp -r "$SCRIPT_DIR/share/limux" "$PREFIX/share/"
fi
echo "$PREFIX/lib/limux" > /etc/ld.so.conf.d/limux.conf
ldconfig 2>/dev/null || true
rm -f "$PREFIX/share/applications/limux.desktop"
install -Dm644 "$SCRIPT_DIR/share/applications/dev.limux.linux.desktop" "$PREFIX/share/applications/dev.limux.linux.desktop"
install -Dm644 "$SCRIPT_DIR/share/metainfo/dev.limux.linux.metainfo.xml" "$PREFIX/share/metainfo/dev.limux.linux.metainfo.xml"
if [ -d "$SCRIPT_DIR/share/icons" ]; then
    cp -r "$SCRIPT_DIR/share/icons/hicolor" "$PREFIX/share/icons/"
fi
gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true

echo ""
echo "Limux installed successfully!"
echo "  Binary:  $PREFIX/bin/limux"
echo "  Library: $PREFIX/lib/limux/libghostty.so"
echo "  Run:     limux"
echo ""
echo "System dependencies (install if missing):"
echo "  sudo apt install libgtk-4-1 libadwaita-1-0 libwebkitgtk-6.0-4"
INSTALL_EOF

chmod 755 "$TARBALL_STAGE/install.sh"
tar -czf "$OUT_DIR/${PKG_BASE}.tar.gz" -C /tmp "${PKG_BASE}"
remove_tree "$TARBALL_STAGE"
echo "  -> dist/${PKG_BASE}.tar.gz"

# =========================================================================
# 2. Debian package
# =========================================================================
echo ""
echo "--- Building .deb ---"
DEB_ROOT="$STAGE/deb"
remove_tree "$DEB_ROOT"
populate_tree "$DEB_ROOT" "/usr"

# ldconfig trigger
mkdir -p "$DEB_ROOT/etc/ld.so.conf.d"
echo "/usr/lib/limux" > "$DEB_ROOT/etc/ld.so.conf.d/limux.conf"

# Control file
INSTALLED_SIZE=$(du -sk "$DEB_ROOT" | cut -f1)
mkdir -p "$DEB_ROOT/DEBIAN"
cat > "$DEB_ROOT/DEBIAN/control" << EOF
Package: limux
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${DEB_ARCH}
Installed-Size: ${INSTALLED_SIZE}
Depends: libgtk-4-1, libadwaita-1-0, libwebkitgtk-6.0-4
Maintainer: Will R <will@limux.dev>
Description: GPU-accelerated terminal workspace manager for Linux
 Limux is a terminal workspace manager powered by Ghostty's
 GPU-rendered terminal engine, with split panes, tabbed workspaces,
 and a built-in browser.
Homepage: https://github.com/am-will/limux
EOF

# Post-install: run ldconfig and update caches
cat > "$DEB_ROOT/DEBIAN/postinst" << 'EOF'
#!/bin/bash
ldconfig 2>/dev/null || true
rm -f /usr/share/applications/limux.desktop
rm -f /usr/local/share/applications/limux.desktop
gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database /usr/share/applications 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postinst"

# Post-remove: clean up
cat > "$DEB_ROOT/DEBIAN/postrm" << 'EOF'
#!/bin/bash
ldconfig 2>/dev/null || true
gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database /usr/share/applications 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postrm"

DEB_FILE="$OUT_DIR/limux_${VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$DEB_FILE"
echo "  -> dist/limux_${VERSION}_${DEB_ARCH}.deb"

# =========================================================================
# 3. RPM package
# =========================================================================
echo ""
echo "--- Building .rpm ---"
build_rpm_package

# =========================================================================
# 4. AppImage
# =========================================================================
echo ""
echo "--- Building AppImage ---"
APPDIR="$STAGE/Limux.AppDir"
remove_tree "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/metainfo" \
         "$APPDIR/usr/share/icons/hicolor/scalable/actions" \
         "$APPDIR/usr/share/limux"

# Binary
cp "$BINARY" "$APPDIR/usr/bin/limux"
strip "$APPDIR/usr/bin/limux"
chmod 755 "$APPDIR/usr/bin/limux"

# Shared library
cp "$GHOSTTY_SO" "$APPDIR/usr/lib/libghostty.so"
strip --strip-debug "$APPDIR/usr/lib/libghostty.so"

# Ghostty resources required for named themes and shell integration
cp -r "$GHOSTTY_SHARE_DIR" "$APPDIR/usr/share/limux/ghostty"

# Desktop file (at AppDir root and in usr/share)
cp "$DESKTOP_FILE" "$APPDIR/dev.limux.linux.desktop"
cp "$DESKTOP_FILE" "$APPDIR/usr/share/applications/dev.limux.linux.desktop"
cp "$METADATA_FILE" "$APPDIR/usr/share/metainfo/dev.limux.linux.metainfo.xml"

# Icons
if [ -d "$ICONS_DIR/hicolor" ]; then
    cp -r "$ICONS_DIR/hicolor/scalable" "$APPDIR/usr/share/icons/hicolor/" 2>/dev/null || true
fi
for svg in "$ICONS_DIR"/*.svg; do
    [ -f "$svg" ] && cp "$svg" "$APPDIR/usr/share/icons/hicolor/scalable/actions/"
done
if [ -d "$APP_ICONS_DIR" ]; then
    for size in 16 32 128 256 512; do
        src="${APP_ICONS_DIR}/${size}.png"
        if [ -f "$src" ]; then
            mkdir -p "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps"
            cp "$src" "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/limux.png"
        fi
    done
fi

# AppImage icon (must be at root as .DirIcon and limux.png)
if [ -f "$APP_ICONS_DIR/256.png" ]; then
    cp "$APP_ICONS_DIR/256.png" "$APPDIR/limux.png"
    cp "$APP_ICONS_DIR/256.png" "$APPDIR/.DirIcon"
fi

# AppRun entry point — sets up library path and launches the binary
cat > "$APPDIR/AppRun" << 'APPRUN_EOF'
#!/bin/bash
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="${HERE}/usr/lib:${LD_LIBRARY_PATH:-}"
export XDG_DATA_DIRS="${HERE}/usr/share:${XDG_DATA_DIRS:-/usr/share}"
exec "${HERE}/usr/bin/limux" "$@"
APPRUN_EOF
chmod 755 "$APPDIR/AppRun"

# Build AppImage
APPIMAGE_FILE="$OUT_DIR/Limux-${VERSION}-${ARCH}.AppImage"
if command -v appimagetool &>/dev/null; then
    APPIMAGETOOL="appimagetool"
elif [ -x /tmp/appimagetool ]; then
    APPIMAGETOOL="/tmp/appimagetool"
else
    echo "WARNING: appimagetool not found, skipping AppImage"
    APPIMAGETOOL=""
fi

if [ -n "$APPIMAGETOOL" ]; then
    ARCH="$ARCH" "$APPIMAGETOOL" "$APPDIR" "$APPIMAGE_FILE" 2>&1 | tail -3
    echo "  -> dist/Limux-${VERSION}-${ARCH}.AppImage"
fi

# =========================================================================
# Summary
# =========================================================================
echo ""
echo "=== Packages created in dist/ ==="
ls -lh "$OUT_DIR"/ 2>/dev/null
echo ""
echo "Install options:"
echo "  Tarball:   tar xzf dist/${PKG_BASE}.tar.gz && cd ${PKG_BASE} && sudo ./install.sh"
echo "  Deb:       sudo dpkg -i ./dist/limux_${VERSION}_${DEB_ARCH}.deb"
echo "  RPM:       sudo rpm -i ./dist/limux-${VERSION}-1.${RPM_ARCH}.rpm"
echo "  AppImage:  chmod +x dist/Limux-${VERSION}-${ARCH}.AppImage && ./dist/Limux-${VERSION}-${ARCH}.AppImage"
