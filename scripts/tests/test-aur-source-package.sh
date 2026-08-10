#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

SOURCE_COMMIT="0123456789abcdef0123456789abcdef01234567"
GHOSTTY_COMMIT="89abcdef0123456789abcdef0123456789abcdef"
OUTPUT="$TMP_DIR/PKGBUILD"

"$ROOT_DIR/scripts/render-aur-source-pkgbuild.sh" \
    "$ROOT_DIR/PKGBUILD-source.template" \
    "$OUTPUT" \
    "1.2.3" \
    "https://github.com/example/limux.git" \
    "$SOURCE_COMMIT" \
    "$GHOSTTY_COMMIT"

bash -n "$OUTPUT"

metadata=$(bash -c '
    set -u
    srcdir=$2
    pkgdir=$3
    source "$1"
    printf "%s\n" "$pkgname" "$pkgver" "$_limux_commit" "$_ghostty_commit" "${source[@]}"
' bash "$OUTPUT" "$TMP_DIR/src" "$TMP_DIR/pkg")

expected=$(printf '%s\n' \
    "limux" \
    "1.2.3" \
    "$SOURCE_COMMIT" \
    "$GHOSTTY_COMMIT" \
    "limux-1.2.3::git+https://github.com/example/limux.git#commit=$SOURCE_COMMIT" \
    "ghostty-1.2.3::git+https://github.com/bvolpato/ghostty.git#commit=$GHOSTTY_COMMIT")
[ "$metadata" = "$expected" ]

if "$ROOT_DIR/scripts/render-aur-source-pkgbuild.sh" \
    "$ROOT_DIR/PKGBUILD-source.template" \
    "$OUTPUT" \
    "not-a-version" \
    "https://github.com/example/limux.git" \
    "$SOURCE_COMMIT" \
    "$GHOSTTY_COMMIT" >/dev/null 2>&1; then
    echo "ERROR: invalid version was accepted" >&2
    exit 1
fi

echo "AUR source package template validation: OK"
