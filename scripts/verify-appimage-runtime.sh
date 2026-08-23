#!/usr/bin/env bash
set -euo pipefail

APPIMAGE_FILE="${1:-}"

if [ -z "$APPIMAGE_FILE" ]; then
    echo "Usage: $0 <AppImage>" >&2
    exit 2
fi

if [ ! -x "$APPIMAGE_FILE" ]; then
    echo "ERROR: AppImage is missing or not executable: $APPIMAGE_FILE" >&2
    exit 1
fi

if ! command -v readelf >/dev/null 2>&1; then
    echo "ERROR: readelf is required to verify the AppImage runtime" >&2
    exit 1
fi

DYNAMIC_SECTION="$(readelf -d "$APPIMAGE_FILE" 2>&1)"
if grep -q '(NEEDED)' <<< "$DYNAMIC_SECTION"; then
    echo "ERROR: AppImage runtime is not statically linked" >&2
    grep '(NEEDED)' <<< "$DYNAMIC_SECTION" >&2
    exit 1
fi

if ! RUNTIME_VERSION="$($APPIMAGE_FILE --appimage-version 2>&1)"; then
    echo "ERROR: AppImage runtime cannot start on the build host" >&2
    echo "$RUNTIME_VERSION" >&2
    exit 1
fi

if ! grep -qi 'AppImage.*version' <<< "$RUNTIME_VERSION"; then
    echo "ERROR: unexpected AppImage runtime version output: $RUNTIME_VERSION" >&2
    exit 1
fi

echo "Verified statically linked AppImage runtime: $RUNTIME_VERSION"
