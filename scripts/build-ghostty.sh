#!/usr/bin/env bash
# Build the pinned embedded library and record the artifact used by local tests.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/check-ghostty.sh"
if [ "$#" -ne 0 ]; then
  echo 'Usage: [ZIG=/path/to/zig] [LIMUX_BUILD_JOBS=4] [LIMUX_GHOSTTY_DEBUG_SYMBOLS=0|1] ./scripts/build-ghostty.sh' >&2
  exit 2
fi
DEBUG_SYMBOLS="${LIMUX_GHOSTTY_DEBUG_SYMBOLS:-0}"
case "$DEBUG_SYMBOLS" in
  0|1) ;;
  *) echo 'ERROR: LIMUX_GHOSTTY_DEBUG_SYMBOLS must be 0 or 1.' >&2; exit 2 ;;
esac
JOBS="${LIMUX_BUILD_JOBS:-4}"
if [[ ! "$JOBS" =~ ^[1-9][0-9]*$ ]]; then
  echo 'ERROR: LIMUX_BUILD_JOBS must be a positive integer.' >&2
  exit 2
fi
ZIG_BIN="$(command -v "${ZIG:-zig}")" || {
  ghostty_build_error "$ROOT_DIR" 'Zig 0.16.0 was not found. Set ZIG to its executable or add it to PATH.'
  exit 1
}
ZIG_BIN="$(realpath "$ZIG_BIN")"
# translate-c discovers the compiler through `zig env`, so nested calls must
# resolve the same Zig installation as the top-level build.
PATH="$(dirname "$ZIG_BIN"):$PATH"
export PATH
if [ "$("$ZIG_BIN" version)" != 0.16.0 ]; then
  ghostty_build_error "$ROOT_DIR" 'This Ghostty checkout requires Zig 0.16.0.'
  exit 1
fi
REVISION="$(ghostty_source_revision "$ROOT_DIR")"
LIB_DIR="$ROOT_DIR/ghostty/zig-out/lib"
LIBRARY="$LIB_DIR/libghostty-internal.so"
PROVENANCE="$LIB_DIR/limux-ghostty-build"
if [ -L "$ROOT_DIR/ghostty/zig-out" ] || [ -L "$LIB_DIR" ]; then
  ghostty_build_error "$ROOT_DIR" 'The Ghostty output directory is a symlink; choose a local output directory before building.'
  exit 1
fi
mkdir -p "$LIB_DIR"
exec 9> "$ROOT_DIR/ghostty/zig-out/.limux-build.lock"
flock 9
# Require fresh output before recording provenance. Unlinking a library symlink
# removes only the link, so the build cannot overwrite an installed library.
rm -f "$PROVENANCE" "$LIBRARY"
ZIG_ARGS=(-Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline -Demit-docs=false -fno-sys=gtk4-layer-shell)
if [ "$DEBUG_SYMBOLS" = 1 ]; then ZIG_ARGS+=(-Dstrip=false); fi
(
  cd "$ROOT_DIR/ghostty"
  unset DESTDIR
  "$ZIG_BIN" build "-j$JOBS" "${ZIG_ARGS[@]}" --prefix "$ROOT_DIR/ghostty/zig-out"
)
if [ "$(ghostty_source_revision "$ROOT_DIR")" != "$REVISION" ]; then
  ghostty_build_error "$ROOT_DIR" 'Ghostty changed revision during the build; no provenance was recorded.'
  exit 1
fi
if [ ! -f "$LIBRARY" ] || [ -L "$LIBRARY" ]; then
  ghostty_build_error "$ROOT_DIR" 'The build did not produce a local Ghostty shared library.'
  exit 1
fi
if [ "$DEBUG_SYMBOLS" = 1 ]; then
  if ! sections="$(readelf --wide --sections "$LIBRARY")" \
    || ! grep -q '\.debug_info ' <<< "$sections" \
    || ! grep -q '\.symtab ' <<< "$sections"; then
    ghostty_build_error "$ROOT_DIR" 'Ghostty build did not retain requested debug symbols.'
    exit 1
  fi
fi
HASH="$(sha256sum < "$LIBRARY")"
TEMP_RECORD="$(mktemp "$PROVENANCE.XXXXXX")"
trap 'rm -f "$TEMP_RECORD"' EXIT
printf '%s\n' \
  limux-ghostty-build-v1 \
  "ghostty_commit=$REVISION" \
  "library_sha256=${HASH%% *}" \
  "library_realpath=$(realpath "$LIBRARY")" \
  zig_version=0.16.0 > "$TEMP_RECORD"
mv "$TEMP_RECORD" "$PROVENANCE"
check_ghostty_build "$ROOT_DIR"
