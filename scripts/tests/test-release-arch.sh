#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/release-arch.sh"

check() {
    local machine="$1" want_arch="$2" want_deb="$3" want_rpm="$4"
    ARCH="" DEB_ARCH="" RPM_ARCH=""
    resolve_release_arch "$machine"
    if [ "$ARCH/$DEB_ARCH/$RPM_ARCH" != "$want_arch/$want_deb/$want_rpm" ]; then
        echo "FAIL: $machine -> $ARCH/$DEB_ARCH/$RPM_ARCH, want $want_arch/$want_deb/$want_rpm" >&2
        exit 1
    fi
}

check x86_64 x86_64 amd64 x86_64
check aarch64 aarch64 arm64 aarch64

for unsupported in arm64 armv7l i686 riscv64 ppc64le; do
    if (resolve_release_arch "$unsupported") >/dev/null 2>&1; then
        echo "FAIL: accepted unsupported architecture: $unsupported" >&2
        exit 1
    fi
done

echo "release architecture mapping: OK"
