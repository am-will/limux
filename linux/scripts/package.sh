#!/usr/bin/env bash
set -euo pipefail

LINUX_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$LINUX_DIR/.." && pwd)"

VERSION="${1:-0.1.0}"
ARCH="$(uname -m)"
PKG_NAME="limux-${VERSION}-linux-${ARCH}"
BUILD_DIR="/tmp/${PKG_NAME}"
TARBALL="${LINUX_DIR}/${PKG_NAME}.tar.gz"
GHOSTTY_SO="${REPO_ROOT}/ghostty/zig-out/lib/libghostty.so"
ICONS_DIR="${LINUX_DIR}/rust/cmux-host-linux/icons"
APP_ICONS_DIR="${REPO_ROOT}/Assets.xcassets/AppIcon.appiconset"
DESKTOP_FILE="${LINUX_DIR}/rust/cmux-host-linux/limux.desktop"

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
cargo build --release --manifest-path "${LINUX_DIR}/Cargo.toml"

BINARY="${LINUX_DIR}/target/release/cmux-linux"
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at ${BINARY}"
    exit 1
fi

# Clean and create staging directory
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"/{lib,share/applications,share/icons/hicolor/scalable/actions}

# Copy binary (renamed to limux)
cp "$BINARY" "$BUILD_DIR/limux"
strip "$BUILD_DIR/limux"
chmod 755 "$BUILD_DIR/limux"

# Copy libghostty.so
cp "$GHOSTTY_SO" "$BUILD_DIR/lib/libghostty.so"
strip --strip-debug "$BUILD_DIR/lib/libghostty.so"

# Copy .desktop file
cp "$DESKTOP_FILE" "$BUILD_DIR/share/applications/limux.desktop"

# Copy action icons (scalable SVGs)
if [ -d "$ICONS_DIR/hicolor" ]; then
    cp -r "$ICONS_DIR/hicolor/scalable" "$BUILD_DIR/share/icons/hicolor/" 2>/dev/null || true
fi
for svg in "$ICONS_DIR"/*.svg; do
    [ -f "$svg" ] && cp "$svg" "$BUILD_DIR/share/icons/hicolor/scalable/actions/"
done

# Copy app launcher icons (PNG, from macOS assets)
if [ -d "$APP_ICONS_DIR" ]; then
    for size in 16 32 128 256 512; do
        src="${APP_ICONS_DIR}/${size}.png"
        if [ -f "$src" ]; then
            dest_dir="$BUILD_DIR/share/icons/hicolor/${size}x${size}/apps"
            mkdir -p "$dest_dir"
            cp "$src" "$dest_dir/limux.png"
        fi
    done
fi

# Generate install.sh
cat > "$BUILD_DIR/install.sh" << 'INSTALL_EOF'
#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr/local"
UNINSTALL=false

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#*=}" ;;
        --prefix)   shift; PREFIX="${1:-/usr/local}" ;;
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

# Binary
install -Dm755 "$SCRIPT_DIR/limux" "$PREFIX/bin/limux"

# Shared library
install -Dm644 "$SCRIPT_DIR/lib/libghostty.so" "$PREFIX/lib/limux/libghostty.so"

# Register library path with ldconfig (belt + suspenders with rpath)
echo "$PREFIX/lib/limux" > /etc/ld.so.conf.d/limux.conf
ldconfig 2>/dev/null || true

# Desktop file
install -Dm644 "$SCRIPT_DIR/share/applications/limux.desktop" "$PREFIX/share/applications/limux.desktop"

# Icons
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

chmod 755 "$BUILD_DIR/install.sh"

# Create tarball
echo "Creating tarball..."
tar -czf "$TARBALL" -C /tmp "$PKG_NAME"
rm -rf "$BUILD_DIR"

SIZE=$(du -h "$TARBALL" | cut -f1)
echo ""
echo "=== Package created ==="
echo "  ${TARBALL} (${SIZE})"
echo ""
echo "To install:"
echo "  tar xzf ${PKG_NAME}.tar.gz"
echo "  cd ${PKG_NAME}"
echo "  sudo ./install.sh"
