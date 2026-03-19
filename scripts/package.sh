#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Read version from Cargo.toml (single source of truth)
VERSION="${1:-$(grep '^version' "$ROOT_DIR/rust/cmux-host-linux/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')}"
ARCH="$(uname -m)"
DEB_ARCH="amd64"
[ "$ARCH" = "aarch64" ] && DEB_ARCH="arm64"

PKG_BASE="limux-${VERSION}-linux-${ARCH}"
STAGE="/tmp/limux-staging"
GHOSTTY_SO="${ROOT_DIR}/ghostty/zig-out/lib/libghostty.so"
ICONS_DIR="${ROOT_DIR}/rust/cmux-host-linux/icons"
APP_ICONS_DIR="${ROOT_DIR}/rust/cmux-host-linux/icons/app"
DESKTOP_FILE="${ROOT_DIR}/rust/cmux-host-linux/limux.desktop"
OUT_DIR="${ROOT_DIR}/dist"

echo "=== Limux Packager ==="
echo "Version: ${VERSION}"
echo "Arch:    ${ARCH}"

# Verify libghostty.so exists
if [ ! -f "$GHOSTTY_SO" ]; then
    echo "ERROR: libghostty.so not found at ${GHOSTTY_SO}"
    echo "Build it first: cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast"
    exit 1
fi

# Build release binary
echo "Building release binary..."
cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

BINARY="${ROOT_DIR}/target/release/cmux-linux"
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at ${BINARY}"
    exit 1
fi

# Clean staging and output
rm -rf "$STAGE" "$OUT_DIR"
mkdir -p "$OUT_DIR"

# =========================================================================
# Helper: populate a prefix tree at a given root
# =========================================================================
populate_tree() {
    local dest="$1"
    local bindir="$dest/usr/local/bin"
    local libdir="$dest/usr/local/lib/limux"
    local appdir="$dest/usr/local/share/applications"
    local icondir="$dest/usr/local/share/icons/hicolor"

    mkdir -p "$bindir" "$libdir" "$appdir" "$icondir/scalable/actions"

    # Binary
    cp "$BINARY" "$bindir/limux"
    strip "$bindir/limux"
    chmod 755 "$bindir/limux"

    # Shared library
    cp "$GHOSTTY_SO" "$libdir/libghostty.so"
    strip --strip-debug "$libdir/libghostty.so"

    # Desktop file
    cp "$DESKTOP_FILE" "$appdir/limux.desktop"

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

# =========================================================================
# 1. Tarball
# =========================================================================
echo ""
echo "--- Building tarball ---"
TARBALL_STAGE="/tmp/${PKG_BASE}"
rm -rf "$TARBALL_STAGE"
mkdir -p "$TARBALL_STAGE"/{lib,share/applications,share/icons/hicolor/scalable/actions}

cp "$BINARY" "$TARBALL_STAGE/limux"
strip "$TARBALL_STAGE/limux"
chmod 755 "$TARBALL_STAGE/limux"
cp "$GHOSTTY_SO" "$TARBALL_STAGE/lib/libghostty.so"
strip --strip-debug "$TARBALL_STAGE/lib/libghostty.so"
cp "$DESKTOP_FILE" "$TARBALL_STAGE/share/applications/limux.desktop"

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

if $UNINSTALL; then
    need_root "$@"
    echo "Uninstalling Limux..."
    rm -f "$PREFIX/bin/limux"
    rm -rf "$PREFIX/lib/limux"
    rm -f /etc/ld.so.conf.d/limux.conf
    ldconfig 2>/dev/null || true
    rm -f "$PREFIX/share/applications/limux.desktop"
    for size in 16 32 128 256 512; do
        rm -f "$PREFIX/share/icons/hicolor/${size}x${size}/apps/limux.png"
    done
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/cmux-globe-symbolic.svg"
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/cmux-split-horizontal-symbolic.svg"
    rm -f "$PREFIX/share/icons/hicolor/scalable/actions/cmux-split-vertical-symbolic.svg"
    gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
    echo "Limux uninstalled."
    exit 0
fi

need_root "$@"
echo "Installing Limux to ${PREFIX}..."

install -Dm755 "$SCRIPT_DIR/limux" "$PREFIX/bin/limux"
install -Dm644 "$SCRIPT_DIR/lib/libghostty.so" "$PREFIX/lib/limux/libghostty.so"
echo "$PREFIX/lib/limux" > /etc/ld.so.conf.d/limux.conf
ldconfig 2>/dev/null || true
install -Dm644 "$SCRIPT_DIR/share/applications/limux.desktop" "$PREFIX/share/applications/limux.desktop"
if [ -d "$SCRIPT_DIR/share/icons" ]; then
    cp -r "$SCRIPT_DIR/share/icons/hicolor" "$PREFIX/share/icons/"
fi
gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true

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
rm -rf "$TARBALL_STAGE"
echo "  -> dist/${PKG_BASE}.tar.gz"

# =========================================================================
# 2. Debian package
# =========================================================================
echo ""
echo "--- Building .deb ---"
DEB_ROOT="$STAGE/deb"
rm -rf "$DEB_ROOT"
populate_tree "$DEB_ROOT"

# ldconfig trigger
mkdir -p "$DEB_ROOT/etc/ld.so.conf.d"
echo "/usr/local/lib/limux" > "$DEB_ROOT/etc/ld.so.conf.d/limux.conf"

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
gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor 2>/dev/null || true
update-desktop-database /usr/local/share/applications 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postinst"

# Post-remove: clean up
cat > "$DEB_ROOT/DEBIAN/postrm" << 'EOF'
#!/bin/bash
ldconfig 2>/dev/null || true
gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor 2>/dev/null || true
update-desktop-database /usr/local/share/applications 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postrm"

DEB_FILE="$OUT_DIR/limux_${VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$DEB_FILE"
echo "  -> dist/limux_${VERSION}_${DEB_ARCH}.deb"

# =========================================================================
# 3. AppImage
# =========================================================================
echo ""
echo "--- Building AppImage ---"
APPDIR="$STAGE/Limux.AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/scalable/actions"

# Binary
cp "$BINARY" "$APPDIR/usr/bin/limux"
strip "$APPDIR/usr/bin/limux"
chmod 755 "$APPDIR/usr/bin/limux"

# Shared library
cp "$GHOSTTY_SO" "$APPDIR/usr/lib/libghostty.so"
strip --strip-debug "$APPDIR/usr/lib/libghostty.so"

# Desktop file (at AppDir root and in usr/share)
cp "$DESKTOP_FILE" "$APPDIR/limux.desktop"
cp "$DESKTOP_FILE" "$APPDIR/usr/share/applications/limux.desktop"

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
echo "  Deb:       sudo apt install ./dist/limux_${VERSION}_${DEB_ARCH}.deb"
echo "  AppImage:  chmod +x dist/Limux-${VERSION}-${ARCH}.AppImage && ./dist/Limux-${VERSION}-${ARCH}.AppImage"
