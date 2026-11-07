#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo 'Usage: install-apparmor-profile.sh --render|--install|--remove /absolute/path/to/limux-host' >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
MODE="$1"
case "$MODE" in --render|--install|--remove) ;; *) usage ;; esac
HOST="$(realpath -m -- "$2")"
if [[ "$2" != /* || ! "$HOST" =~ ^/[a-zA-Z0-9_./+-]+$ ]]; then
    echo 'ERROR: the host path must be absolute and contain only letters, digits, /, _, ., +, or -.' >&2
    exit 2
fi
PROFILE="limux-$(printf '%s' "$HOST" | sha256sum | cut -c1-24)"
PROFILE_FILE="/etc/apparmor.d/$PROFILE"

render() {
    cat <<EOF
abi <abi/4.0>,
include <tunables/global>

profile $PROFILE "$HOST" flags=(unconfined) {
  userns,
}
EOF
}

if [ "$MODE" = --render ]; then
    render
    exit 0
fi
if [ "$(id -u)" -ne 0 ]; then
    echo 'ERROR: installing or removing an AppArmor allowance requires root.' >&2
    exit 1
fi
if [ "$MODE" = --remove ]; then
    if [ -f "$PROFILE_FILE" ]; then
        if command -v apparmor_parser >/dev/null && command -v aa-enabled >/dev/null && aa-enabled --quiet; then
            apparmor_parser --remove "$PROFILE_FILE" || true
        fi
        rm -f -- "$PROFILE_FILE"
    fi
    exit 0
fi
if ! command -v apparmor_parser >/dev/null || ! command -v aa-enabled >/dev/null \
   || ! aa-enabled --quiet || [ ! -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]; then
    echo 'AppArmor user-namespace restrictions are unavailable; no allowance is needed.'
    exit 0
fi
if [ ! -x "$HOST" ]; then
    echo "ERROR: host executable is missing: $HOST" >&2
    exit 1
fi
TEMP_PROFILE="$(mktemp)"
trap 'rm -f "$TEMP_PROFILE"' EXIT
render > "$TEMP_PROFILE"
apparmor_parser --skip-kernel-load --skip-cache "$TEMP_PROFILE"
install -m644 "$TEMP_PROFILE" "$PROFILE_FILE"
apparmor_parser --replace "$PROFILE_FILE"
echo "Installed WebKit user-namespace allowance for $HOST"
