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
expected_custom_profile="limux-$(printf '%s' "$(realpath -m -- "$TEST_DIR/install/limux-host")" | sha256sum | cut -c1-24)"
grep -Fq "profile $expected_custom_profile \"$(realpath -m -- "$TEST_DIR/install/limux-host")\" flags=(unconfined)" "$TEST_DIR/real-profile"

# System host path must render the canonical packaged profile rather than a conflicting limux-<hash> attachment.
"$RENDERER" --render /usr/libexec/limux/limux-host > "$TEST_DIR/system-profile"
"$RENDERER" --render /usr/libexec/limux/../limux/limux-host > "$TEST_DIR/system-relative-profile"
cmp "$ROOT_DIR/scripts/apparmor/dev.limux.linux" "$TEST_DIR/system-profile"
cmp "$ROOT_DIR/scripts/apparmor/dev.limux.linux" "$TEST_DIR/system-relative-profile"

for path in relative/path '/tmp/limux*' '/tmp/limux[ab]' '/tmp/limux{one,two}' '/tmp/limux"' $'/tmp/limux\nuserns,'; do
    if "$RENDERER" --render "$path" > "$TEST_DIR/rejected" 2>/dev/null; then
        echo "FAIL: accepted an unsafe AppArmor attachment: $path" >&2
        exit 1
    fi
done

# Verify --install / --remove and package post-install scripts clean up legacy hashed system profiles.
LEGACY_HASH="limux-$(printf '%s' /usr/libexec/limux/limux-host | sha256sum | cut -c1-24)"
[ "$LEGACY_HASH" = "limux-8c15f0a5f176c1d3593ba948" ]
SANDBOX="$TEST_DIR/sandbox"
FAKE_BIN="$TEST_DIR/bin"
PARSER_LOG="$TEST_DIR/apparmor-parser.log"
mkdir -p "$SANDBOX/etc/apparmor.d" "$SANDBOX/proc/sys/kernel" "$SANDBOX/usr/libexec/limux" "$FAKE_BIN"
cp /bin/true "$SANDBOX/usr/libexec/limux/limux-host"
printf '1\n' > "$SANDBOX/proc/sys/kernel/apparmor_restrict_unprivileged_userns"

cat > "$FAKE_BIN/id" <<'SH'
#!/bin/sh
echo 0
SH
cat > "$FAKE_BIN/aa-enabled" <<'SH'
#!/bin/sh
exit 0
SH
cat > "$FAKE_BIN/apparmor_parser" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >> "$LIMUX_TEST_PARSER_LOG"
SH
chmod 755 "$FAKE_BIN/id" "$FAKE_BIN/aa-enabled" "$FAKE_BIN/apparmor_parser"

sed \
    -e "s|/etc/apparmor.d|$SANDBOX/etc/apparmor.d|g" \
    -e "s|/proc/sys/kernel/apparmor_restrict_unprivileged_userns|$SANDBOX/proc/sys/kernel/apparmor_restrict_unprivileged_userns|g" \
    -e "s|\\[ ! -x \"\\\$HOST\" \\]|[ ! -x \"$SANDBOX\\\$HOST\" ]|g" \
    "$RENDERER" > "$TEST_DIR/sandboxed-installer.sh"
chmod 755 "$TEST_DIR/sandboxed-installer.sh"

printf 'legacy profile\n' > "$SANDBOX/etc/apparmor.d/$LEGACY_HASH"
: > "$PARSER_LOG"
PATH="$FAKE_BIN:$PATH" LIMUX_TEST_PARSER_LOG="$PARSER_LOG" \
    "$TEST_DIR/sandboxed-installer.sh" --install /usr/libexec/limux/limux-host >/dev/null
[ ! -e "$SANDBOX/etc/apparmor.d/$LEGACY_HASH" ]
cmp "$ROOT_DIR/scripts/apparmor/dev.limux.linux" "$SANDBOX/etc/apparmor.d/dev.limux.linux"
expected_install_calls="$(printf '%s\n' \
    "--remove $SANDBOX/etc/apparmor.d/$LEGACY_HASH" \
    "--replace $SANDBOX/etc/apparmor.d/dev.limux.linux")"
actual_install_calls="$(grep -E '^--(remove|replace) ' "$PARSER_LOG")"
[ "$actual_install_calls" = "$expected_install_calls" ]

printf 'legacy profile\n' > "$SANDBOX/etc/apparmor.d/$LEGACY_HASH"
: > "$PARSER_LOG"
PATH="$FAKE_BIN:$PATH" LIMUX_TEST_PARSER_LOG="$PARSER_LOG" \
    "$TEST_DIR/sandboxed-installer.sh" --remove /usr/libexec/limux/limux-host >/dev/null
[ ! -e "$SANDBOX/etc/apparmor.d/$LEGACY_HASH" ]
[ ! -e "$SANDBOX/etc/apparmor.d/dev.limux.linux" ]
expected_remove_calls="$(printf '%s\n' \
    "--remove $SANDBOX/etc/apparmor.d/$LEGACY_HASH" \
    "--remove $SANDBOX/etc/apparmor.d/dev.limux.linux")"
[ "$(cat "$PARSER_LOG")" = "$expected_remove_calls" ]

# Exercise the exact AppArmor migration blocks from .deb postinst and .rpm %post.
awk '
    /^legacy_apparmor_profile=/ { capture = 1 }
    capture { print }
    capture && /^fi$/ && seen_replace { exit }
    capture && /apparmor_parser --replace/ { seen_replace = 1 }
' "$ROOT_DIR/scripts/package.sh" | sed \
    -e "s|/etc/apparmor.d|$SANDBOX/etc/apparmor.d|g" \
    -e "s|/proc/sys/kernel/apparmor_restrict_unprivileged_userns|$SANDBOX/proc/sys/kernel/apparmor_restrict_unprivileged_userns|g" \
    > "$TEST_DIR/deb-postinst-apparmor.sh"

awk '
    /^legacy_apparmor_profile=/ { capture = 1 }
    capture { print }
    capture && /^fi$/ && seen_replace { exit }
    capture && /apparmor_parser --replace/ { seen_replace = 1 }
' "$ROOT_DIR/scripts/limux.spec" | sed \
    -e "s|/etc/apparmor.d|$SANDBOX/etc/apparmor.d|g" \
    -e "s|/proc/sys/kernel/apparmor_restrict_unprivileged_userns|$SANDBOX/proc/sys/kernel/apparmor_restrict_unprivileged_userns|g" \
    > "$TEST_DIR/rpm-post-apparmor.sh"

for snippet in "$TEST_DIR/deb-postinst-apparmor.sh" "$TEST_DIR/rpm-post-apparmor.sh"; do
    cp "$ROOT_DIR/scripts/apparmor/dev.limux.linux" "$SANDBOX/etc/apparmor.d/dev.limux.linux"
    printf 'legacy profile\n' > "$SANDBOX/etc/apparmor.d/$LEGACY_HASH"
    : > "$PARSER_LOG"
    PATH="$FAKE_BIN:$PATH" LIMUX_TEST_PARSER_LOG="$PARSER_LOG" bash -euo pipefail "$snippet"
    [ ! -e "$SANDBOX/etc/apparmor.d/$LEGACY_HASH" ]
    [ "$(cat "$PARSER_LOG")" = "$expected_install_calls" ]
done

if command -v apparmor_parser >/dev/null; then
    apparmor_parser --skip-kernel-load --skip-cache "$TEST_DIR/real-profile"
    apparmor_parser --skip-kernel-load --skip-cache "$TEST_DIR/system-profile"
    apparmor_parser --skip-kernel-load --skip-cache "$ROOT_DIR/scripts/apparmor/dev.limux.linux"
fi
echo 'AppArmor exact-path profile validation: OK'
