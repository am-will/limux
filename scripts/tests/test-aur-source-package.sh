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
    printf "%s\n" \
        "$pkgname" "$pkgver" "$_limux_commit" "$_ghostty_commit" \
        "${makedepends[@]}" "${source[@]}" "${sha256sums[@]}"
' bash "$OUTPUT" "$TMP_DIR/src" "$TMP_DIR/pkg")

expected=$(printf '%s\n' \
    "limux" \
    "1.2.3" \
    "$SOURCE_COMMIT" \
    "$GHOSTTY_COMMIT" \
    "git" \
    "libepoxy" \
    "rust" \
    "zig-x86_64-linux-0.16.0.tar.xz::https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz" \
    "limux-1.2.3::git+https://github.com/example/limux.git#commit=$SOURCE_COMMIT" \
    "ghostty-1.2.3::git+https://github.com/bvolpato/ghostty.git#commit=$GHOSTTY_COMMIT" \
    "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00" \
    "SKIP" \
    "SKIP")

# Verify build() explicitly selects the pinned compiler over a rolling system Zig.
mkdir -p "$TMP_DIR/src/limux-1.2.3/scripts" \
    "$TMP_DIR/src/zig-x86_64-linux-0.16.0" "$TMP_DIR/system-bin"
cat > "$TMP_DIR/src/zig-x86_64-linux-0.16.0/zig" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = version ]; then
    printf '%s\n' 0.16.0
    exit 0
fi
STUB
cat > "$TMP_DIR/system-bin/zig" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = version ]; then
    printf '%s\n' 0.17.0
    exit 0
fi
STUB
cat > "$TMP_DIR/src/limux-1.2.3/scripts/build-ghostty.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
selected_zig=$(command -v "${ZIG:-zig}")
version=$("$selected_zig" version)
printf '%s\n' "$selected_zig" "$version" > "$TEST_ZIG_SELECTION"
if [[ "$version" != 0.16.0 ]]; then
    echo "Selected Zig $version instead of 0.16.0" >&2
    exit 1
fi
STUB
cat > "$TMP_DIR/system-bin/cargo" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x \
    "$TMP_DIR/src/zig-x86_64-linux-0.16.0/zig" \
    "$TMP_DIR/system-bin/zig" \
    "$TMP_DIR/src/limux-1.2.3/scripts/build-ghostty.sh" \
    "$TMP_DIR/system-bin/cargo"
export TEST_ZIG_SELECTION="$TMP_DIR/zig-selection"
PATH="$TMP_DIR/system-bin:$PATH" bash -c '
    set -euo pipefail
    srcdir=$1
    source "$2"
    build
' bash "$TMP_DIR/src" "$OUTPUT"
selected=$(cat "$TEST_ZIG_SELECTION")
expected_selection=$(printf '%s\n' "$TMP_DIR/src/zig-x86_64-linux-0.16.0/zig" 0.16.0)
[ "$selected" = "$expected_selection" ]
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
