#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_DIR="$(mktemp -d -t limux-user-install-XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT
REAL_INSTALL_PATH="$(command -v install)"
REAL_ID_PATH="$(command -v id)"
REAL_UID="$("$REAL_ID_PATH" -u)"

PACKAGE_DIR="$TMP_DIR/package"
BACKTICK="$(printf '\140')"
PREFIX="$TMP_DIR/user prefix & pipe|quote\" dollar\$ backtick${BACKTICK} backslash\\launcher"
SYSTEM_ROOT="$TMP_DIR/system"
FAKE_BIN="$TMP_DIR/bin"
ROOT_GUARD_BIN="$TMP_DIR/root-guard-bin"
SUDO_LOG="$TMP_DIR/sudo.log"
SIDE_EFFECT_LOG="$TMP_DIR/global-side-effects.log"
FAILURES=0

fail() {
    echo "FAIL: $*" >&2
    FAILURES=$((FAILURES + 1))
}

launch_desktop_and_check() {
    local desktop_file="$1"
    local proof_file="$2"
    local expected_executable="$3"
    local attempt

    if ! LIMUX_TEST_LAUNCH_PROOF="$proof_file" gio launch "$desktop_file"; then
        fail "GIO could not launch desktop entry: $desktop_file"
        return
    fi
    for ((attempt = 0; attempt < 50; attempt++)); do
        if [ -f "$proof_file" ]; then
            [ "$(cat "$proof_file")" = "$expected_executable" ] || \
                fail "GIO executed an unexpected Limux entrypoint: $(cat "$proof_file")"
            return
        fi
        sleep 0.1
    done
    fail "GIO did not execute the launcher for: $desktop_file"
}

run_locked_prefix_install() {
    local sudo_log="$1"
    local output="$2"
    local prefix="$3"

    if [ "$REAL_UID" -eq 0 ]; then
        command -v setpriv >/dev/null 2>&1 || return 1
        chmod 755 "$TMP_DIR" || return 1
        chown -R 65534:65534 "$prefix" || return 1
        setpriv --reuid=65534 --regid=65534 --clear-groups \
            env PATH="$PATH" \
                LIMUX_TEST_SUDO_LOG="$sudo_log" \
                LIMUX_TEST_SUDO_FAIL=1 \
                "$PACKAGE_DIR/install.sh" --prefix="$prefix" > "$output" 2>&1
    else
        LIMUX_TEST_SUDO_LOG="$sudo_log" \
        LIMUX_TEST_SUDO_FAIL=1 \
            "$PACKAGE_DIR/install.sh" --prefix="$prefix" > "$output" 2>&1
    fi
}

mkdir -p "$PACKAGE_DIR/libexec/limux" "$PACKAGE_DIR/lib" \
    "$PACKAGE_DIR/share/limux/ghostty" \
    "$PACKAGE_DIR/share/icons/hicolor/scalable/actions" \
    "$PACKAGE_DIR/share/applications" "$PACKAGE_DIR/share/metainfo" \
    "$SYSTEM_ROOT/usr/local/libexec/limux" "$SYSTEM_ROOT/usr/libexec/limux" \
    "$SYSTEM_ROOT/usr/local/bin" "$SYSTEM_ROOT/usr/bin" \
    "$SYSTEM_ROOT/etc/ld.so.conf.d" "$FAKE_BIN" "$ROOT_GUARD_BIN"

# Pull out the installer that package.sh ships, then map its fixed system paths
# into this test's temporary tree so the test cannot touch the host system.
awk '
    /^cat > .*install\.sh.*<< / { capture = 1; next }
    capture && /^INSTALL_EOF$/ { exit }
    capture { print }
' "$ROOT_DIR/scripts/package.sh" > "$TMP_DIR/install.sh"
sed \
    -e "s|/usr|$SYSTEM_ROOT/usr|g" \
    -e "s|$SYSTEM_ROOT/usr/bin/env|/usr/bin/env|g" \
    -e "s|/etc/ld.so.conf.d|$SYSTEM_ROOT/etc/ld.so.conf.d|g" \
    "$TMP_DIR/install.sh" > "$PACKAGE_DIR/install.sh"
chmod 755 "$PACKAGE_DIR/install.sh"

cat > "$PACKAGE_DIR/limux" <<'SH'
#!/bin/sh
if [ -n "${LIMUX_TEST_LAUNCH_PROOF:-}" ]; then
    printf '%s\n' "$0" > "$LIMUX_TEST_LAUNCH_PROOF"
fi
echo "limux CLI"
SH
cat > "$PACKAGE_DIR/libexec/limux/limux-host" <<'SH'
#!/bin/sh
echo "GApplication options"
SH
chmod 755 "$PACKAGE_DIR/limux" "$PACKAGE_DIR/libexec/limux/limux-host"
printf 'synthetic Ghostty library\n' > "$PACKAGE_DIR/lib/libghostty-internal.so"
printf 'Ghostty resource sentinel\n' > "$PACKAGE_DIR/share/limux/ghostty/installer-layout-test"
printf '<svg/>' > "$PACKAGE_DIR/share/icons/hicolor/scalable/actions/limux-globe-symbolic.svg"
cat > "$PACKAGE_DIR/share/applications/dev.limux.linux.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Limux Test
Exec=limux
TryExec=limux
EOF
printf '<component/>' > "$PACKAGE_DIR/share/metainfo/dev.limux.linux.metainfo.xml"

# These stand in for old system installs. A custom-prefix install must leave
# every one of them untouched, including the shared ldconfig file.
printf 'legacy libexec sentinel\n' > "$SYSTEM_ROOT/usr/local/libexec/limux/limux"
printf 'legacy libexec sentinel\n' > "$SYSTEM_ROOT/usr/libexec/limux/limux"
for path in "$SYSTEM_ROOT/usr/local/bin/limux" "$SYSTEM_ROOT/usr/bin/limux"; do
    cat > "$path" <<'SH'
#!/bin/sh
echo "GApplication options"
SH
    chmod 755 "$path"
done
printf '%s\n' "$SYSTEM_ROOT/usr/local/lib/limux" > "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"

cat > "$FAKE_BIN/id" <<'SH'
#!/bin/sh
if [ -n "${LIMUX_TEST_SUDO_REEXEC:-}" ]; then
    echo 0
else
    echo 1000
fi
SH
cat > "$FAKE_BIN/sudo" <<'SH'
#!/bin/sh
printf 'sudo invoked\n' >> "$LIMUX_TEST_SUDO_LOG"
if [ -n "${LIMUX_TEST_SUDO_FAIL:-}" ]; then
    exit 88
fi
LIMUX_TEST_SUDO_REEXEC=1 exec "$@"
SH
cat > "$FAKE_BIN/ldconfig" <<'SH'
#!/bin/sh
printf 'ldconfig %s\n' "$*" >> "$LIMUX_TEST_SIDE_EFFECT_LOG"
SH
cat > "$FAKE_BIN/appstreamcli" <<'SH'
#!/bin/sh
printf 'appstreamcli %s\n' "$*" >> "$LIMUX_TEST_SIDE_EFFECT_LOG"
SH
for command in gtk-update-icon-cache update-desktop-database; do
    cat > "$FAKE_BIN/$command" <<'SH'
#!/bin/sh
exit 0
SH
done
chmod 755 "$FAKE_BIN"/*
cat > "$FAKE_BIN/install" <<'SH'
#!/bin/sh
for last_arg do :; done
case "$last_arg" in
    "$LIMUX_TEST_ALLOWED_ROOT"/*)
        exec "$LIMUX_TEST_REAL_INSTALL" "$@"
        ;;
    *)
        echo "refusing install target outside test fixture: $last_arg" >&2
        exit 99
        ;;
esac
SH
chmod 755 "$FAKE_BIN/install"
export LIMUX_TEST_ALLOWED_ROOT="$TMP_DIR"
export LIMUX_TEST_REAL_INSTALL="$REAL_INSTALL_PATH"
export PATH="$FAKE_BIN:$PATH"
export LIMUX_TEST_SUDO_LOG="$SUDO_LOG"
export LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG"

cat > "$ROOT_GUARD_BIN/sudo" <<'SH'
#!/bin/sh
printf 'sudo invoked for root alias\n' >> "$LIMUX_TEST_SUDO_LOG"
exit 99
SH
chmod 755 "$ROOT_GUARD_BIN/sudo"
ROOT_ALIAS="$TMP_DIR/../.."
if PATH="$ROOT_GUARD_BIN:$FAKE_BIN:$PATH" \
    LIMUX_TEST_SUDO_LOG="$SUDO_LOG" \
    LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG" \
    "$PACKAGE_DIR/install.sh" --prefix="$ROOT_ALIAS" > "$TMP_DIR/root-prefix.log" 2>&1; then
    fail "filesystem-root alias was accepted as an install prefix"
fi
grep -Fq "Install prefix cannot be the filesystem root." "$TMP_DIR/root-prefix.log" || \
    fail "filesystem-root alias was not rejected before installation"
[ ! -e "$SUDO_LOG" ] || fail "filesystem-root alias attempted privilege escalation"
rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"

PERCENT_PREFIX="$TMP_DIR/percent%prefix"
if "$PACKAGE_DIR/install.sh" --prefix="$PERCENT_PREFIX" > "$TMP_DIR/percent-prefix.log" 2>&1; then
    fail "percent-containing install prefix was accepted"
fi
grep -Fq "Install prefix cannot contain '%' because this installer does not support it in desktop executable paths." \
    "$TMP_DIR/percent-prefix.log" || fail "percent-containing install prefix was not rejected clearly"
[ ! -e "$PERCENT_PREFIX" ] || fail "rejected percent prefix was created before validation"
[ ! -e "$SUDO_LOG" ] || fail "rejected percent prefix attempted privilege escalation"
[ ! -s "$SIDE_EFFECT_LOG" ] || fail "rejected percent prefix caused side effects"
rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"

"$PACKAGE_DIR/install.sh" --prefix="$PREFIX"

[ -x "$PREFIX/bin/limux" ] || fail "user-prefix install did not create the CLI"
[ -x "$PREFIX/libexec/limux/limux-host" ] || fail "user-prefix install did not create the host"
[ -f "$PREFIX/share/limux/ghostty/installer-layout-test" ] || \
    fail "user-prefix install did not preserve the Limux resource directory layout"
[ ! -e "$PREFIX/share/ghostty/installer-layout-test" ] || \
    fail "user-prefix install copied Limux resources into the wrong share directory"
ICON_FILE="$PREFIX/share/icons/hicolor/scalable/actions/limux-globe-symbolic.svg"
[ -f "$ICON_FILE" ] || fail "user-prefix install did not preserve the hicolor icon directory layout"
[ ! -e "$PREFIX/share/icons/scalable/actions/limux-globe-symbolic.svg" ] || \
    fail "user-prefix install copied hicolor icons into the wrong share directory"
DESKTOP_FILE="$PREFIX/share/applications/dev.limux.linux.desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$DESKTOP_FILE"
fi
if command -v gio >/dev/null 2>&1; then
    launch_desktop_and_check "$DESKTOP_FILE" "$TMP_DIR/space-prefix-launch-proof" "$PREFIX/bin/limux"
else
    fail "gio is required to verify desktop launcher behavior"
fi

for sentinel in \
    "$SYSTEM_ROOT/usr/local/libexec/limux/limux" \
    "$SYSTEM_ROOT/usr/libexec/limux/limux" \
    "$SYSTEM_ROOT/usr/local/bin/limux" \
    "$SYSTEM_ROOT/usr/bin/limux" \
    "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"; do
    [ -e "$sentinel" ] || fail "user-prefix install removed global sentinel: $sentinel"
done
[ ! -e "$SUDO_LOG" ] || fail "writable user-prefix install invoked sudo"
[ ! -s "$SIDE_EFFECT_LOG" ] || fail "user-prefix install ran a global cache command: $(cat "$SIDE_EFFECT_LOG")"

rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
"$PACKAGE_DIR/install.sh" --prefix="$PREFIX" --uninstall

[ ! -e "$PREFIX/bin/limux" ] || fail "user-prefix uninstall left the CLI behind"
[ ! -e "$PREFIX/libexec/limux" ] || fail "user-prefix uninstall left the host directory behind"
[ ! -e "$PREFIX/lib/limux" ] || fail "user-prefix uninstall left the library directory behind"
[ ! -e "$PREFIX/share/limux/ghostty/installer-layout-test" ] || \
    fail "user-prefix uninstall left installed Limux resources behind"
[ ! -e "$PREFIX/share/ghostty/installer-layout-test" ] || \
    fail "user-prefix uninstall left resources copied into the wrong directory"
[ ! -e "$ICON_FILE" ] || fail "user-prefix uninstall left the installed hicolor icon behind"
[ ! -e "$PREFIX/share/icons/scalable/actions/limux-globe-symbolic.svg" ] || \
    fail "user-prefix uninstall left icons copied into the wrong directory"
for sentinel in \
    "$SYSTEM_ROOT/usr/local/libexec/limux/limux" \
    "$SYSTEM_ROOT/usr/libexec/limux/limux" \
    "$SYSTEM_ROOT/usr/local/bin/limux" \
    "$SYSTEM_ROOT/usr/bin/limux" \
    "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"; do
    [ -e "$sentinel" ] || fail "user-prefix uninstall removed global sentinel: $sentinel"
done
[ ! -e "$SUDO_LOG" ] || fail "writable user-prefix uninstall invoked sudo"
[ ! -s "$SIDE_EFFECT_LOG" ] || fail "user-prefix uninstall ran a global cache command: $(cat "$SIDE_EFFECT_LOG")"

LOCKED_PREFIX="$TMP_DIR/locked user prefix"
mkdir -p "$LOCKED_PREFIX/bin" "$LOCKED_PREFIX/libexec/limux"
chmod 777 "$LOCKED_PREFIX" "$LOCKED_PREFIX/bin" "$LOCKED_PREFIX/libexec"
printf 'old CLI sentinel\n' > "$LOCKED_PREFIX/bin/limux"
printf 'old host sentinel\n' > "$LOCKED_PREFIX/libexec/limux/limux-host"
chmod 755 "$LOCKED_PREFIX/bin/limux" "$LOCKED_PREFIX/libexec/limux/limux-host"
chmod 555 "$LOCKED_PREFIX/libexec/limux"
LOCKED_SUDO_LOG="$LOCKED_PREFIX/sudo.log"
LOCKED_OUTPUT="$LOCKED_PREFIX/install.log"
if run_locked_prefix_install "$LOCKED_SUDO_LOG" "$LOCKED_OUTPUT" "$LOCKED_PREFIX"; then
    fail "locked managed destination did not request escalation"
fi
[ -s "$LOCKED_SUDO_LOG" ] || fail "locked managed destination did not invoke sudo before writing"
[ "$(cat "$LOCKED_PREFIX/bin/limux")" = "old CLI sentinel" ] || \
    fail "locked destination changed the CLI before escalation"
[ "$(cat "$LOCKED_PREFIX/libexec/limux/limux-host")" = "old host sentinel" ] || \
    fail "locked destination changed the host before escalation"
chmod 755 "$LOCKED_PREFIX/libexec/limux"

LOCKED_ICON="$LOCKED_PREFIX/share/icons/hicolor/scalable/actions/limux-globe-symbolic.svg"
mkdir -p "${LOCKED_ICON%/*}"
printf 'old icon sentinel\n' > "$LOCKED_ICON"
chmod 444 "$LOCKED_ICON"
rm -f "$LOCKED_SUDO_LOG"
if run_locked_prefix_install "$LOCKED_SUDO_LOG" "$LOCKED_PREFIX/icon-install.log" "$LOCKED_PREFIX"; then
    fail "read-only Limux icon did not request escalation"
fi
[ -s "$LOCKED_SUDO_LOG" ] || fail "read-only Limux icon did not invoke sudo before writing"
[ "$(cat "$LOCKED_PREFIX/bin/limux")" = "old CLI sentinel" ] || \
    fail "locked icon update changed the CLI before escalation"
[ "$(cat "$LOCKED_PREFIX/libexec/limux/limux-host")" = "old host sentinel" ] || \
    fail "locked icon update changed the host before escalation"
[ "$(cat "$LOCKED_ICON")" = "old icon sentinel" ] || \
    fail "locked icon update changed the icon before escalation"

LEGACY_USER_PREFIX="$TMP_DIR/legacy-user-prefix"
printf '%s\n' "$LEGACY_USER_PREFIX/lib/limux" > "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"
"$PACKAGE_DIR/install.sh" --prefix="$LEGACY_USER_PREFIX"

[ -x "$LEGACY_USER_PREFIX/bin/limux" ] || fail "legacy user-prefix install did not create the CLI"
[ "$(cat "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf")" = "$LEGACY_USER_PREFIX/lib/limux" ] || \
    fail "unprivileged install changed its matching legacy loader config"
[ ! -e "$SUDO_LOG" ] || fail "matching legacy user-prefix install invoked sudo"
if grep -q '^ldconfig ' "$SIDE_EFFECT_LOG" 2>/dev/null; then
    fail "unprivileged matching legacy install ran ldconfig"
fi

rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
"$PACKAGE_DIR/install.sh" --prefix="$LEGACY_USER_PREFIX" --uninstall

[ ! -e "$LEGACY_USER_PREFIX/bin/limux" ] || fail "legacy user-prefix uninstall left the CLI behind"
[ "$(cat "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf")" = "$LEGACY_USER_PREFIX/lib/limux" ] || \
    fail "unprivileged uninstall changed its matching legacy loader config"
[ ! -e "$SUDO_LOG" ] || fail "matching legacy user-prefix uninstall invoked sudo"
if grep -q '^ldconfig ' "$SIDE_EFFECT_LOG" 2>/dev/null; then
    fail "unprivileged matching legacy uninstall ran ldconfig"
fi

printf '%s\n' "$SYSTEM_ROOT/usr/local/lib/limux" > "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"
SYSTEM_PREFIX="$SYSTEM_ROOT/usr"
rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
PATH="$FAKE_BIN:$SYSTEM_ROOT/usr/local/bin:$PATH" \
SUDO_USER="" \
LIMUX_TEST_SUDO_LOG="$SUDO_LOG" \
LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG" \
"$PACKAGE_DIR/install.sh" --prefix="$SYSTEM_PREFIX" > "$TMP_DIR/usr-install.log"

[ -x "$SYSTEM_PREFIX/bin/limux" ] || fail "system-prefix install did not create the CLI"
[ -s "$SUDO_LOG" ] || fail "system-prefix install did not request root"
[ "$(cat "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf")" = "$SYSTEM_ROOT/usr/local/lib/limux" ] || \
    fail "system-prefix install changed another prefix's loader config"
for sentinel in \
    "$SYSTEM_ROOT/usr/local/libexec/limux/limux" \
    "$SYSTEM_ROOT/usr/local/bin/limux"; do
    [ -e "$sentinel" ] || fail "system-prefix install touched another prefix's sentinel: $sentinel"
done
[ "$("$SYSTEM_PREFIX/bin/limux" --help 2>&1)" = "limux CLI" ] || \
    fail "system-prefix install did not replace the old entrypoint with the CLI"
[ ! -e "$SYSTEM_PREFIX/libexec/limux/limux" ] || \
    fail "system-prefix install left its legacy host"
if ! grep -Fq "WARNING: the first limux on PATH is '$SYSTEM_ROOT/usr/local/bin/limux', expected '$SYSTEM_PREFIX/bin/limux'." "$TMP_DIR/usr-install.log"; then
    fail "system-prefix install did not warn about the other prefix shadowing limux"
fi
if grep -q '^ldconfig ' "$SIDE_EFFECT_LOG" 2>/dev/null; then
    fail "system-prefix install ran ldconfig for another prefix's config"
fi

rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
PATH="$FAKE_BIN:$PATH" \
SUDO_USER="" \
LIMUX_TEST_SUDO_LOG="$SUDO_LOG" \
LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG" \
"$PACKAGE_DIR/install.sh" --prefix="$SYSTEM_PREFIX" --uninstall

[ ! -e "$SYSTEM_PREFIX/bin/limux" ] || fail "system-prefix uninstall left the CLI behind"
[ "$(cat "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf")" = "$SYSTEM_ROOT/usr/local/lib/limux" ] || \
    fail "system-prefix uninstall removed another prefix's loader config"
[ -s "$SUDO_LOG" ] || fail "system-prefix uninstall did not request root"
if grep -q '^ldconfig ' "$SIDE_EFFECT_LOG" 2>/dev/null; then
    fail "system-prefix uninstall ran ldconfig for another prefix's config"
fi

# Exercise the other system prefix while retaining the first one's sentinels.
SYSTEM_PREFIX="$SYSTEM_ROOT/usr/local"
mkdir -p "$SYSTEM_PREFIX/libexec/limux" "$SYSTEM_PREFIX/bin"
printf 'legacy libexec sentinel\n' > "$SYSTEM_PREFIX/libexec/limux/limux"
cat > "$SYSTEM_PREFIX/bin/limux" <<'SH'
#!/bin/sh
echo "GApplication options"
SH
chmod 755 "$SYSTEM_PREFIX/bin/limux"
mkdir -p "$SYSTEM_ROOT/usr/libexec/limux" "$SYSTEM_ROOT/usr/bin"
printf 'other prefix libexec sentinel\n' > "$SYSTEM_ROOT/usr/libexec/limux/limux"
cat > "$SYSTEM_ROOT/usr/bin/limux" <<'SH'
#!/bin/sh
echo "GApplication options"
SH
chmod 755 "$SYSTEM_ROOT/usr/bin/limux"
printf '%s\n' "$SYSTEM_PREFIX/lib/limux" > "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf"

rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
PATH="$FAKE_BIN:$SYSTEM_ROOT/usr/bin:$PATH" \
SUDO_USER="" \
LIMUX_TEST_SUDO_LOG="$SUDO_LOG" \
LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG" \
"$PACKAGE_DIR/install.sh" --prefix="$SYSTEM_PREFIX" > "$TMP_DIR/usr-local-install.log"

[ -x "$SYSTEM_PREFIX/bin/limux" ] || fail "/usr/local-prefix install did not create the CLI"
[ ! -e "$SYSTEM_PREFIX/libexec/limux/limux" ] || \
    fail "/usr/local-prefix install left its legacy host"
[ "$("$SYSTEM_PREFIX/bin/limux" --help 2>&1)" = "limux CLI" ] || \
    fail "/usr/local-prefix install did not replace its old entrypoint with the CLI"
if ! grep -Fq "WARNING: the first limux on PATH is '$SYSTEM_ROOT/usr/bin/limux', expected '$SYSTEM_PREFIX/bin/limux'." "$TMP_DIR/usr-local-install.log"; then
    fail "/usr/local-prefix install did not warn about the other prefix shadowing limux"
fi
[ ! -e "$SYSTEM_ROOT/etc/ld.so.conf.d/limux.conf" ] || \
    fail "/usr/local-prefix install did not remove its matching legacy loader config"
for sentinel in "$SYSTEM_ROOT/usr/libexec/limux/limux" "$SYSTEM_ROOT/usr/bin/limux"; do
    [ -e "$sentinel" ] || fail "/usr/local-prefix install touched /usr sentinel: $sentinel"
done
if [ ! -s "$SIDE_EFFECT_LOG" ] || ! grep -q '^ldconfig ' "$SIDE_EFFECT_LOG"; then
    fail "/usr/local-prefix install did not refresh the cache after removing its legacy config"
fi

rm -f "$SUDO_LOG" "$SIDE_EFFECT_LOG"
PATH="$FAKE_BIN:$PATH" \
SUDO_USER="" \
LIMUX_TEST_SUDO_LOG="$SUDO_LOG" \
LIMUX_TEST_SIDE_EFFECT_LOG="$SIDE_EFFECT_LOG" \
"$PACKAGE_DIR/install.sh" --prefix="$SYSTEM_PREFIX" --uninstall

[ ! -e "$SYSTEM_PREFIX/bin/limux" ] || fail "/usr/local-prefix uninstall left the CLI behind"
for sentinel in "$SYSTEM_ROOT/usr/libexec/limux/limux" "$SYSTEM_ROOT/usr/bin/limux"; do
    [ -e "$sentinel" ] || fail "/usr/local-prefix uninstall touched /usr sentinel: $sentinel"
done
[ -s "$SUDO_LOG" ] || fail "/usr/local-prefix uninstall did not request root"
if grep -q '^ldconfig ' "$SIDE_EFFECT_LOG" 2>/dev/null; then
    fail "/usr/local-prefix uninstall ran ldconfig without removing its config"
fi

if [ "$FAILURES" -ne 0 ]; then
    exit 1
fi

echo "user-prefix install and uninstall validation: OK"
