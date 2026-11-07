#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
RENDERER="$ROOT_DIR/scripts/install-apparmor-profile.sh"
TEST_DIR="$(mktemp -d -t limux-apparmor-test-XXXXXX)"
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$TEST_DIR/install"
cp /bin/true "$TEST_DIR/install/limux-host"
ln -s install/limux-host "$TEST_DIR/limux-host-link"
"$RENDERER" --render "$TEST_DIR/install/limux-host" > "$TEST_DIR/real-profile"
"$RENDERER" --render "$TEST_DIR/limux-host-link" > "$TEST_DIR/link-profile"
cmp "$TEST_DIR/real-profile" "$TEST_DIR/link-profile"
for path in relative/path '/tmp/limux*' '/tmp/limux[ab]' '/tmp/limux{one,two}' '/tmp/limux"' $'/tmp/limux\nuserns,'; do
    if "$RENDERER" --render "$path" > "$TEST_DIR/rejected" 2>/dev/null; then
        echo "FAIL: accepted an unsafe AppArmor attachment: $path" >&2
        exit 1
    fi
done
if command -v apparmor_parser >/dev/null; then
    apparmor_parser --skip-kernel-load --skip-cache "$TEST_DIR/real-profile"
    apparmor_parser --skip-kernel-load --skip-cache "$ROOT_DIR/scripts/apparmor/dev.limux.linux"
fi
echo 'AppArmor exact-path profile validation: OK'
