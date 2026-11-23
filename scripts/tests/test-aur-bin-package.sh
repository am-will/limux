#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

RENDER="$ROOT_DIR/scripts/render-aur-bin-pkgbuild.sh"
TEMPLATE="$ROOT_DIR/PKGBUILD.template"
OUTPUT="$TMP_DIR/PKGBUILD"
SHA_X86_64="$(printf '%064d' 1)"
SHA_AARCH64="$(printf '%064d' 2)"
URL_BASE="https://github.com/am-will/limux/releases/download/v1.2.3"

"$RENDER" "$TEMPLATE" "$OUTPUT" 1.2.3 "$SHA_X86_64" "$SHA_AARCH64"
bash -n "$OUTPUT"

metadata=$(bash -c '
    set -u
    source "$1"
    printf "%s\n" "$pkgname" "$pkgver" "${arch[@]}" \
        "${source_x86_64[@]}" "${sha256sums_x86_64[@]}" \
        "${source_aarch64[@]}" "${sha256sums_aarch64[@]}"
    # No arch-independent sources: each arch downloads only its own tarball.
    [ -z "${source+x}" ] && [ -z "${sha256sums+x}" ]
' bash "$OUTPUT")

expected=$(printf '%s\n' \
    "limux-bin" \
    "1.2.3" \
    "x86_64" \
    "aarch64" \
    "limux-1.2.3-x86_64.tar.gz::$URL_BASE/limux-1.2.3-linux-x86_64.tar.gz" \
    "$SHA_X86_64" \
    "limux-1.2.3-aarch64.tar.gz::$URL_BASE/limux-1.2.3-linux-aarch64.tar.gz" \
    "$SHA_AARCH64")
[ "$metadata" = "$expected" ]

# package() must install from the tarball directory matching CARCH.
for carch in x86_64 aarch64; do
    srcdir="$TMP_DIR/src-$carch"
    pkgdir="$TMP_DIR/pkg-$carch"
    tree="$srcdir/limux-1.2.3-linux-$carch"
    mkdir -p \
        "$tree/libexec/limux" \
        "$tree/lib" \
        "$tree/share/applications" \
        "$tree/share/metainfo" \
        "$tree/share/icons/hicolor/16x16/apps" \
        "$tree/share/limux" \
        "$pkgdir"
    printf '%s\n' "$carch" > "$tree/limux"
    printf 'host\n' > "$tree/libexec/limux/limux-host"
    printf 'ghostty\n' > "$tree/lib/libghostty-internal.so"
    printf '[Desktop Entry]\n' > "$tree/share/applications/dev.limux.linux.desktop"
    printf '<component/>\n' > "$tree/share/metainfo/dev.limux.linux.metainfo.xml"
    printf 'png\n' > "$tree/share/icons/hicolor/16x16/apps/limux.png"
    printf 'data\n' > "$tree/share/limux/README"

    (
        cd "$srcdir"
        bash -c '
            set -euo pipefail
            pkgdir=$1
            CARCH=$2
            source "$3"
            package
        ' bash "$pkgdir" "$carch" "$OUTPUT"
    )
    [ "$(cat "$pkgdir/usr/bin/limux")" = "$carch" ]
    [ -x "$pkgdir/usr/libexec/limux/limux-host" ]
    [ -f "$pkgdir/usr/lib/limux/libghostty-internal.so" ]
done

for args in \
    "not-a-version $SHA_X86_64 $SHA_AARCH64" \
    "1.2.3 not-a-sha $SHA_AARCH64" \
    "1.2.3 $SHA_X86_64 not-a-sha"; do
    # shellcheck disable=SC2086
    if "$RENDER" "$TEMPLATE" "$OUTPUT" $args >/dev/null 2>&1; then
        echo "ERROR: invalid render arguments were accepted: $args" >&2
        exit 1
    fi
done

echo "AUR binary package template validation: OK"
