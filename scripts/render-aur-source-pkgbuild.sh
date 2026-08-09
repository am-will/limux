#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 6 ]; then
    echo "Usage: $0 <template> <output> <version> <source-repository> <source-commit> <ghostty-commit>" >&2
    exit 2
fi

TEMPLATE="$1"
OUTPUT="$2"
VERSION="$3"
SOURCE_REPOSITORY="$4"
SOURCE_COMMIT="$5"
GHOSTTY_COMMIT="$6"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "ERROR: invalid version: $VERSION" >&2
    exit 1
fi
if [[ ! "$SOURCE_REPOSITORY" =~ ^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(\.git)?$ ]]; then
    echo "ERROR: invalid GitHub source repository: $SOURCE_REPOSITORY" >&2
    exit 1
fi
if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]]; then
    echo "ERROR: invalid source commit: $SOURCE_COMMIT" >&2
    exit 1
fi
if [[ ! "$GHOSTTY_COMMIT" =~ ^[0-9a-f]{40}$ ]]; then
    echo "ERROR: invalid Ghostty commit: $GHOSTTY_COMMIT" >&2
    exit 1
fi

sed \
    -e "s|@@VERSION@@|$VERSION|g" \
    -e "s|@@SOURCE_REPOSITORY@@|$SOURCE_REPOSITORY|g" \
    -e "s|@@SOURCE_COMMIT@@|$SOURCE_COMMIT|g" \
    -e "s|@@GHOSTTY_COMMIT@@|$GHOSTTY_COMMIT|g" \
    "$TEMPLATE" > "$OUTPUT"

if grep -q '@@[A-Z_]*@@' "$OUTPUT"; then
    echo "ERROR: unresolved placeholder in $OUTPUT" >&2
    exit 1
fi
