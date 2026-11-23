#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 5 ]; then
    echo "Usage: $0 <template> <output> <version> <x86_64-tarball-sha256> <aarch64-tarball-sha256>" >&2
    exit 2
fi

TEMPLATE="$1"
OUTPUT="$2"
VERSION="$3"
SHA256_X86_64="$4"
SHA256_AARCH64="$5"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "ERROR: invalid version: $VERSION" >&2
    exit 1
fi
for sha in "$SHA256_X86_64" "$SHA256_AARCH64"; do
    if [[ ! "$sha" =~ ^[0-9a-f]{64}$ ]]; then
        echo "ERROR: invalid sha256: $sha" >&2
        exit 1
    fi
done

sed \
    -e "s|@@VERSION@@|$VERSION|g" \
    -e "s|@@SHA256_X86_64@@|$SHA256_X86_64|g" \
    -e "s|@@SHA256_AARCH64@@|$SHA256_AARCH64|g" \
    "$TEMPLATE" > "$OUTPUT"

if grep -q '@@[A-Z0-9_]*@@' "$OUTPUT"; then
    echo "ERROR: unresolved placeholder in $OUTPUT" >&2
    exit 1
fi
