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
WEBKITGTK_RUNTIME_DIR=""
WEBKITGTK_PROCESS_DIR=""
ICONS_DIR="${ROOT_DIR}/rust/limux-host-linux/icons"
APP_ICONS_DIR="${ROOT_DIR}/rust/limux-host-linux/icons/app"
DESKTOP_FILE="${ROOT_DIR}/rust/limux-host-linux/dev.limux.linux.desktop"
METADATA_FILE="${ROOT_DIR}/rust/limux-host-linux/dev.limux.linux.metainfo.xml"
OUT_DIR="${ROOT_DIR}/dist"
GHOSTTY_ZIG_ARGS=(-Doptimize=ReleaseFast -Dcpu=baseline)
CLI_ENTRYPOINT_NAME="limux"
HOST_ENTRYPOINT_NAME="limux-host"

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

    required_glibc="$(glibc_requirement_for "$path" || true)"
    if [ -z "$required_glibc" ]; then
        echo "WARNING: unable to determine GLIBC requirement for ${label}"
        return 0
    fi

    if version_gt "$required_glibc" "$MAX_GLIBC_VERSION"; then
        echo "ERROR: ${label} requires GLIBC_${required_glibc}, which exceeds the supported release baseline GLIBC_${MAX_GLIBC_VERSION}."
        echo "Build release artifacts inside an environment pinned to GLIBC_${MAX_GLIBC_VERSION}."
        echo "Override the baseline intentionally with LIMUX_MAX_GLIBC=<version> if you are targeting a newer distro on purpose."
        exit 1
    fi

    echo "Verified ${label} GLIBC requirement: GLIBC_${required_glibc} (target max GLIBC_${MAX_GLIBC_VERSION})"
}

assert_cli_entrypoint() {
    local path="$1"
    local label="$2"

    if ! "$path" --help 2>&1 | grep -q "limux CLI"; then
        echo "ERROR: ${label} is not the limux CLI entrypoint: ${path}"
        exit 1
    fi
}

assert_no_legacy_host_entrypoint() {
    local path="$1"
    local label="$2"

    if [ -e "$path" ]; then
        echo "ERROR: ${label} contains legacy host entrypoint at ${path}"
        echo "Only the CLI may be named 'limux'; the GTK host must be 'limux-host'."
        exit 1
    fi
}

assert_pixbuf_svg_loader_bundle() {
    local appdir="$1"
    local loader_dir_rel="$2"
    local cache_dir_rel="$3"

    if ! ls "${appdir}/${loader_dir_rel}/"libpixbufloader[-_]svg.so >/dev/null 2>&1; then
        echo "ERROR: AppImage is missing libpixbufloader-svg.so under ${loader_dir_rel}/"
        exit 1
    fi
    if [ ! -f "${appdir}/usr/lib/librsvg-2.so.2" ]; then
        echo "ERROR: AppImage is missing librsvg-2.so.2 (loader closure copy failed?)"
        exit 1
    fi
    if [ ! -s "${appdir}/${cache_dir_rel}/loaders.cache.template" ]; then
        echo "ERROR: AppImage is missing loaders.cache.template under ${cache_dir_rel}/"
        exit 1
    fi
    if ! grep -q "@LOADER_DIR@" "${appdir}/${cache_dir_rel}/loaders.cache.template"; then
        echo "ERROR: loaders.cache.template missing @LOADER_DIR@ placeholder — AppRun substitution will fail"
        exit 1
    fi
    echo "Verified bundled SVG pixbuf loader, librsvg-2.so.2, and loaders.cache.template"
}

install_desktop_file() {
    local src="$1"
    local dest="$2"
    local exec_path="$3"

    sed \
        -e "s|^Exec=.*|Exec=${exec_path}|" \
        -e "s|^TryExec=.*|TryExec=${exec_path}|" \
        "$src" > "$dest"
    chmod 644 "$dest"
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

. "${ROOT_DIR}/scripts/appimage-webkit.sh"

configure_ghostty_build_args() {
    if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists gtk4-layer-shell-0; then
        echo "gtk4-layer-shell not available via pkg-config; building Ghostty with bundled gtk4-layer-shell."
        GHOSTTY_ZIG_ARGS+=(-fno-sys=gtk4-layer-shell)
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
            "${GHOSTTY_ZIG_ARGS[@]}" \
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

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 not found in PATH."
    echo "Install Python 3, then rerun ./scripts/package.sh"
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
configure_ghostty_build_args
echo "Building libghostty (ReleaseFast, cpu=baseline)..."
(cd "${ROOT_DIR}/ghostty" && zig build -Dapp-runtime=none "${GHOSTTY_ZIG_ARGS[@]}")
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

if ! WEBKITGTK_RUNTIME_DIR="$(resolve_webkitgtk_runtime_dir)"; then
    echo "ERROR: WebKitGTK 6 runtime directory not found."
    echo "Install the runtime/development package before building release artifacts:"
    echo "  Ubuntu/Debian: sudo apt install libwebkitgtk-6.0-dev"
    echo "  Fedora:        sudo dnf install webkitgtk6.0-devel"
    exit 1
fi

if ! WEBKITGTK_PROCESS_DIR="$(resolve_webkitgtk_process_dir)"; then
    echo "ERROR: WebKitGTK 6 helper processes not found."
    echo "Expected WebKitWebProcess from the WebKitGTK runtime package."
    exit 1
fi

# Build release binary
echo "Building release binary..."
cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

CLI_BINARY="${ROOT_DIR}/target/release/limux-cli"
HOST_BINARY="${ROOT_DIR}/target/release/limux"
if [ ! -f "$CLI_BINARY" ]; then
    echo "ERROR: CLI binary not found at ${CLI_BINARY}"
    exit 1
fi
if [ ! -f "$HOST_BINARY" ]; then
    echo "ERROR: Host binary not found at ${HOST_BINARY}"
    exit 1
fi

assert_glibc_compatibility "$GHOSTTY_SO" "libghostty.so"
assert_glibc_compatibility "$CLI_BINARY" "limux CLI"
assert_glibc_compatibility "$HOST_BINARY" "limux host"
assert_cli_entrypoint "$CLI_BINARY" "target/release/limux-cli"

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
    local libexecdir="$dest${prefix}/libexec/limux"
    local libdir="$dest${prefix}/lib/limux"
    local ghostty_datadir="$dest${prefix}/share/limux"
    local ghostty_resdir="$ghostty_datadir/ghostty"
    local appdir="$dest${prefix}/share/applications"
    local metadatadir="$dest${prefix}/share/metainfo"
    local icondir="$dest${prefix}/share/icons/hicolor"

    mkdir -p "$bindir" "$libexecdir" "$libdir" "$ghostty_resdir" "$appdir" "$metadatadir" "$icondir/scalable/actions"

    # Public CLI and private GTK host binary.
    cp "$CLI_BINARY" "$bindir/$CLI_ENTRYPOINT_NAME"
    cp "$HOST_BINARY" "$libexecdir/$HOST_ENTRYPOINT_NAME"
    rm -f "$libexecdir/limux"
    if [ "$strip_files" = "true" ]; then
        strip "$bindir/$CLI_ENTRYPOINT_NAME"
        strip "$libexecdir/$HOST_ENTRYPOINT_NAME"
    fi
    chmod 755 "$bindir/$CLI_ENTRYPOINT_NAME" "$libexecdir/$HOST_ENTRYPOINT_NAME"
    assert_cli_entrypoint "$bindir/$CLI_ENTRYPOINT_NAME" "packaged $prefix/bin/$CLI_ENTRYPOINT_NAME"
    assert_no_legacy_host_entrypoint "$libexecdir/limux" "packaged $prefix libexec tree"

    # Shared library
    cp "$GHOSTTY_SO" "$libdir/libghostty.so"
    if [ "$strip_files" = "true" ]; then
        strip --strip-debug "$libdir/libghostty.so"
    fi

    # Ghostty resources required for named themes and shell integration
    cp -r "$GHOSTTY_SHARE_DIR"/. "$ghostty_resdir"
    copy_ghostty_terminfo_entries "$GHOSTTY_TERMINFO_DIR" "$ghostty_datadir/terminfo"

    # Desktop file. Use the absolute CLI path so desktop launchers do not
    # accidentally resolve an older GTK host binary named `limux` from PATH.
    install_desktop_file "$DESKTOP_FILE" "$appdir/dev.limux.linux.desktop" "$prefix/bin/$CLI_ENTRYPOINT_NAME"
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
mkdir -p "$TARBALL_STAGE"/{lib,libexec/limux,share/limux/ghostty,share/limux/terminfo,share/applications,share/icons/hicolor/scalable/actions}
mkdir -p "$TARBALL_STAGE/share/metainfo"

cp "$CLI_BINARY" "$TARBALL_STAGE/limux"
cp "$HOST_BINARY" "$TARBALL_STAGE/libexec/limux/limux-host"
strip "$TARBALL_STAGE/limux"
strip "$TARBALL_STAGE/libexec/limux/limux-host"
chmod 755 "$TARBALL_STAGE/limux" "$TARBALL_STAGE/libexec/limux/limux-host"
assert_cli_entrypoint "$TARBALL_STAGE/limux" "tarball limux"
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

install_desktop_file() {
    local src="$1"
    local dest="$2"
    local exec_path="$3"

    sed \
        -e "s|^Exec=.*|Exec=${exec_path}|" \
        -e "s|^TryExec=.*|TryExec=${exec_path}|" \
        "$src" > "$dest"
    chmod 644 "$dest"
}

legacy_limux_paths() {
    local sudo_home=""

    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        sudo_home="$(getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6 || true)"
    fi

    printf '%s\n' \
        "$PREFIX/libexec/limux/limux" \
        /usr/local/libexec/limux/limux \
        /usr/libexec/limux/limux \
        /usr/local/bin/limux \
        /usr/bin/limux

    if [ -n "$sudo_home" ]; then
        printf '%s\n' \
            "$sudo_home/.local/libexec/limux/limux" \
            "$sudo_home/.local/bin/limux"
    fi
}

is_legacy_limux_host() {
    local path="$1"
    local help

    [ -x "$path" ] || return 1
    help="$("$path" --help 2>&1 || true)"
    printf '%s\n' "$help" | grep -q "limux CLI" && return 1
    printf '%s\n' "$help" | grep -q "GApplication" && return 0
    "$path" --json identify >/tmp/limux-installer-probe.log 2>&1 && return 1
    grep -q "Unknown option --json" /tmp/limux-installer-probe.log
}

clean_legacy_limux_entrypoints() {
    local path

    while IFS= read -r path; do
        [ -n "$path" ] || continue
        [ "$path" = "$PREFIX/bin/limux" ] && continue
        if [ "${path%/bin/limux}" != "$path" ]; then
            if is_legacy_limux_host "$path"; then
                rm -f "$path"
                echo "Removed legacy Limux host entrypoint: $path"
            fi
        elif [ -e "$path" ]; then
            rm -f "$path"
            echo "Removed legacy Limux host entrypoint: $path"
        fi
    done <<EOF_PATHS
$(legacy_limux_paths)
EOF_PATHS
}

warn_if_limux_is_shadowed() {
    local expected="$PREFIX/bin/limux"
    local first

    first="$(PATH="$PREFIX/bin:$PATH" command -v limux 2>/dev/null || true)"
    if [ "$first" != "$expected" ]; then
        echo "WARNING: the first limux on PATH is '$first', expected '$expected'."
        echo "         Agent/CLI commands require the Limux CLI entrypoint."
    fi

    if ! "$expected" --help 2>&1 | grep -q "limux CLI"; then
        echo "ERROR: installed limux entrypoint is not the CLI: $expected" >&2
        exit 1
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
    remove_tree "$PREFIX/libexec/limux"
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
clean_legacy_limux_entrypoints
install -Dm755 "$SCRIPT_DIR/libexec/limux/limux-host" "$PREFIX/libexec/limux/limux-host"
install -Dm644 "$SCRIPT_DIR/lib/libghostty.so" "$PREFIX/lib/limux/libghostty.so"
if [ -d "$SCRIPT_DIR/share/limux" ]; then
    cp -r "$SCRIPT_DIR/share/limux" "$PREFIX/share/"
fi
echo "$PREFIX/lib/limux" > /etc/ld.so.conf.d/limux.conf
ldconfig 2>/dev/null || true
rm -f "$PREFIX/share/applications/limux.desktop"
mkdir -p "$PREFIX/share/applications"
install_desktop_file "$SCRIPT_DIR/share/applications/dev.limux.linux.desktop" "$PREFIX/share/applications/dev.limux.linux.desktop" "$PREFIX/bin/limux"
install -Dm644 "$SCRIPT_DIR/share/metainfo/dev.limux.linux.metainfo.xml" "$PREFIX/share/metainfo/dev.limux.linux.metainfo.xml"
if [ -d "$SCRIPT_DIR/share/icons" ]; then
    cp -r "$SCRIPT_DIR/share/icons/hicolor" "$PREFIX/share/icons/"
fi
gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true
warn_if_limux_is_shadowed

echo ""
echo "Limux installed successfully!"
echo "  CLI:     $PREFIX/bin/limux"
echo "  Host:    $PREFIX/libexec/limux/limux-host"
echo "  Library: $PREFIX/lib/limux/libghostty.so"
echo "  App:     limux"
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
set -e

is_legacy_limux_host() {
    path="$1"
    [ -x "$path" ] || return 1
    help="$("$path" --help 2>&1 || true)"
    echo "$help" | grep -q "limux CLI" && return 1
    echo "$help" | grep -q "GApplication" && return 0
    "$path" --json identify >/tmp/limux-postinst-probe.log 2>&1 && return 1
    grep -q "Unknown option --json" /tmp/limux-postinst-probe.log
}

ldconfig 2>/dev/null || true
rm -f /usr/libexec/limux/limux
rm -f /usr/local/libexec/limux/limux
if is_legacy_limux_host /usr/local/bin/limux; then
    rm -f /usr/local/bin/limux
fi
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
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/libexec/limux" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/metainfo" \
         "$APPDIR/usr/share/icons/hicolor/scalable/actions" \
         "$APPDIR/usr/share/limux"

# Public CLI and private GTK host binary.
cp "$CLI_BINARY" "$APPDIR/usr/bin/limux"
cp "$HOST_BINARY" "$APPDIR/usr/libexec/limux/limux-host"
strip "$APPDIR/usr/bin/limux"
strip "$APPDIR/usr/libexec/limux/limux-host"
chmod 755 "$APPDIR/usr/bin/limux" "$APPDIR/usr/libexec/limux/limux-host"
assert_cli_entrypoint "$APPDIR/usr/bin/limux" "AppImage usr/bin/limux"

# Shared library
cp "$GHOSTTY_SO" "$APPDIR/usr/lib/libghostty.so"
strip --strip-debug "$APPDIR/usr/lib/libghostty.so"

# WebKitGTK runtime, helper processes, and non-glibc library dependencies.
copy_appimage_webkit_runtime "$APPDIR"

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

# Bundle gdk-pixbuf SVG loader + librsvg closure. Without these, symbolic SVG
# icons (the pane action toolbar) fail to render on hosts that don't ship
# rsvg-pixbuf-loader — e.g. Fedora 44+, which dropped the package — because
# the AppImage's bundled libgdk_pixbuf has no loader to delegate to.
# See https://github.com/am-will/limux/issues/80
PIXBUF_LOADER_DIR_REL="usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
PIXBUF_CACHE_DIR_REL="usr/lib/gdk-pixbuf-2.0/2.10.0"
PIXBUF_SVG_LOADER=""

# Map `uname -m` to the Debian multiarch tuple. dpkg-architecture is the
# authoritative source on Debian/Ubuntu; the case statement is a fallback for
# hosts without dpkg (Fedora, Arch, …) where the multiarch path is unused
# anyway. Also covers /usr/lib/gdk-pixbuf-2.0/... (no arch infix) used by Arch.
PIXBUF_MULTIARCH=""
if command -v dpkg-architecture >/dev/null 2>&1; then
    PIXBUF_MULTIARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || true)"
fi
if [ -z "$PIXBUF_MULTIARCH" ]; then
    case "$(uname -m)" in
        x86_64) PIXBUF_MULTIARCH="x86_64-linux-gnu" ;;
        aarch64) PIXBUF_MULTIARCH="aarch64-linux-gnu" ;;
        i386|i486|i586|i686) PIXBUF_MULTIARCH="i386-linux-gnu" ;;
        armv7l|armv7) PIXBUF_MULTIARCH="arm-linux-gnueabihf" ;;
        armv6l) PIXBUF_MULTIARCH="arm-linux-gnueabi" ;;
        *) PIXBUF_MULTIARCH="$(uname -m)-linux-gnu" ;;
    esac
fi

for candidate in \
    /usr/lib/${PIXBUF_MULTIARCH}/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so \
    /usr/lib/${PIXBUF_MULTIARCH}/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader_svg.so \
    /usr/lib64/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so \
    /usr/lib64/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader_svg.so \
    /usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so \
    /usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader_svg.so
do
    if [ -f "$candidate" ]; then
        PIXBUF_SVG_LOADER="$candidate"
        break
    fi
done

if [ -n "$PIXBUF_SVG_LOADER" ]; then
    mkdir -p "$APPDIR/$PIXBUF_LOADER_DIR_REL"
    cp "$PIXBUF_SVG_LOADER" "$APPDIR/$PIXBUF_LOADER_DIR_REL/"

    # Drag in librsvg-2 and its closure — the loader dlopens it at runtime.
    copy_appimage_library_closure "$APPDIR/usr/lib" "$PIXBUF_SVG_LOADER"

    # Generate a relocatable loaders.cache template. AppRun substitutes
    # @LOADER_DIR@ with the live mount path at runtime. Use a tab delimiter
    # for sed since neither the AppDir path nor "@LOADER_DIR@" can contain
    # a literal tab.
    # Debian/Ubuntu ship `gdk-pixbuf-query-loaders` under the multiarch lib
    # dir, not in $PATH — check there before falling back to PATH.
    QUERY_LOADERS=""
    for candidate in \
        /usr/lib/${PIXBUF_MULTIARCH}/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders \
        /usr/lib64/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders \
        /usr/lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders
    do
        if [ -x "$candidate" ]; then
            QUERY_LOADERS="$candidate"
            break
        fi
    done
    if [ -z "$QUERY_LOADERS" ]; then
        if command -v gdk-pixbuf-query-loaders >/dev/null 2>&1; then
            QUERY_LOADERS=gdk-pixbuf-query-loaders
        elif command -v gdk-pixbuf-query-loaders-64 >/dev/null 2>&1; then
            QUERY_LOADERS=gdk-pixbuf-query-loaders-64
        fi
    fi

    if [ -n "$QUERY_LOADERS" ]; then
        GDK_PIXBUF_MODULEDIR="$APPDIR/$PIXBUF_LOADER_DIR_REL" "$QUERY_LOADERS" \
            | sed -e $'s\t'"$APPDIR/$PIXBUF_LOADER_DIR_REL"$'\t@LOADER_DIR@\tg' \
            > "$APPDIR/$PIXBUF_CACHE_DIR_REL/loaders.cache.template"
    else
        echo "WARNING: gdk-pixbuf-query-loaders not found; AppImage SVG loader not registered."
        echo "         Install libgdk-pixbuf2.0-bin (Debian/Ubuntu) or gdk-pixbuf2-modules (Fedora)."
        echo "         AppImage will ship without SVG symbolic icons working on hosts without rsvg-pixbuf-loader."
        if [ "${LIMUX_REQUIRE_SVG_LOADER:-}" = "1" ]; then
            exit 1
        fi
    fi

    # Smoke check: assert the loader bundle is wired correctly (skipped if
    # the query step warned and continued without producing the template).
    if [ -s "$APPDIR/$PIXBUF_CACHE_DIR_REL/loaders.cache.template" ]; then
        assert_pixbuf_svg_loader_bundle "$APPDIR" "$PIXBUF_LOADER_DIR_REL" "$PIXBUF_CACHE_DIR_REL"
    fi
else
    echo "WARNING: libpixbufloader-svg.so not found on build host."
    echo "         Install rsvg-pixbuf-loader (Fedora), librsvg2-common (Debian/Ubuntu),"
    echo "         or librsvg (Arch) to bundle the loader. AppImage will ship without"
    echo "         SVG symbolic icons working on hosts that don't ship the loader (e.g. Fedora 44+)."
    echo "         Set LIMUX_REQUIRE_SVG_LOADER=1 to make this a hard error (used by official CI)."
    if [ "${LIMUX_REQUIRE_SVG_LOADER:-}" = "1" ]; then
        exit 1
    fi
fi

# AppRun entry point — sets up library path and launches the binary
cat > "$APPDIR/AppRun" << 'APPRUN_EOF'
#!/bin/bash
HERE="$(dirname "$(readlink -f "$0")")"
cd "$HERE"
export LD_LIBRARY_PATH="${HERE}/usr/lib:${LD_LIBRARY_PATH:-}"
export XDG_DATA_DIRS="${HERE}/usr/share:${XDG_DATA_DIRS:-/usr/share}"

# Activate bundled gdk-pixbuf SVG loader by materializing loaders.cache with
# the current mount path. Written to $XDG_CACHE_HOME so it works on a
# read-only FUSE-mounted AppImage. See packaging step that bundles the loader.
# Only export GDK_PIXBUF_MODULE_FILE if mkdir and sed both succeed — otherwise
# pointing at a missing/stale cache silently breaks SVG rendering.
PIXBUF_DIR="${HERE}/usr/lib/gdk-pixbuf-2.0/2.10.0"
if [ -f "${PIXBUF_DIR}/loaders.cache.template" ] && [ -d "${PIXBUF_DIR}/loaders" ]; then
    LIMUX_CACHE_BASE="${XDG_CACHE_HOME:-$HOME/.cache}"
    LIMUX_CACHE="${LIMUX_CACHE_BASE}/limux"
    PIXBUF_CACHE="${LIMUX_CACHE}/pixbuf-loaders.cache.$$"
    if [ -n "${LIMUX_CACHE_BASE}" ] \
       && mkdir -p "$LIMUX_CACHE" 2>/dev/null \
       && sed -e $'s\t@LOADER_DIR@\t'"${PIXBUF_DIR}/loaders"$'\tg' \
              "${PIXBUF_DIR}/loaders.cache.template" > "${PIXBUF_CACHE}" 2>/dev/null \
       && [ -s "${PIXBUF_CACHE}" ]; then
        export GDK_PIXBUF_MODULE_FILE="${PIXBUF_CACHE}"
    fi
    unset LIMUX_CACHE_BASE LIMUX_CACHE PIXBUF_CACHE
fi
unset PIXBUF_DIR

export WEBKIT_EXEC_PATH="${HERE}/usr/lib/webkitgtk-6.0"
export WEBKIT_INJECTED_BUNDLE_PATH="${HERE}/usr/lib/webkitgtk-6.0/injected-bundle"
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
