#!/usr/bin/env bash
# Give only this CI command a temporary user-namespace allowance.
set -euo pipefail

DENY=false
if [ "${1:-}" = --deny-userns ]; then
    DENY=true
    shift
fi
[ "${1:-}" = -- ] && shift
if [ "$#" -eq 0 ]; then
    echo 'Usage: ci-webkit-sandbox.sh [--deny-userns] -- command [arguments...]' >&2
    exit 2
fi
for dependency in aa-exec apparmor_parser bwrap sudo; do
    command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency" >&2; exit 2; }
done
RESTRICTION="$(sysctl -n kernel.apparmor_restrict_unprivileged_userns)"
if [ "$RESTRICTION" != 1 ]; then
    echo "ERROR: CI must retain Ubuntu's AppArmor user-namespace restriction at 1; got $RESTRICTION." >&2
    exit 1
fi
sysctl kernel.apparmor_restrict_unprivileged_userns kernel.unprivileged_userns_clone user.max_user_namespaces
printf 'Caller AppArmor label: '
cat /proc/self/attr/current
bwrap --version
PROFILE_DIR="$(mktemp -d -t limux-ci-apparmor-XXXXXX)"
PROFILE="limux-ci-${PROFILE_DIR##*-}"
LOADED=false
cleanup() {
    local result=$?
    trap - EXIT
    if [ "$result" -ne 0 ]; then
        sudo journalctl -k -n 100 --no-pager 2>/dev/null | grep -E 'apparmor|userns|bwrap' || true
    fi
    if $LOADED; then
        sudo apparmor_parser --remove "$PROFILE_DIR/profile" || result=1
    fi
    rm -rf -- "$PROFILE_DIR"
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
USERNS='userns,'
$DENY && USERNS='deny userns,'
cat > "$PROFILE_DIR/profile" <<EOF
abi <abi/4.0>,
include <tunables/global>
profile $PROFILE flags=(unconfined) {
  $USERNS
}
EOF
sudo apparmor_parser --replace --skip-cache "$PROFILE_DIR/profile"
LOADED=true
printf 'Temporary command profile: %s\n' "$PROFILE"
aa-exec -p "$PROFILE" -- cat /proc/self/attr/current
if aa-exec -p "$PROFILE" -- bwrap --ro-bind / / --proc /proc --dev /dev --unshare-user --uid 0 --gid 0 -- /bin/true; then
    if $DENY; then
        echo 'ERROR: negative profile unexpectedly permitted the real bubblewrap namespace probe.' >&2
        exit 1
    fi
elif ! $DENY; then
    echo 'ERROR: temporary application allowance did not permit the real bubblewrap namespace probe.' >&2
    exit 1
fi
aa-exec -p "$PROFILE" -- "$@"
[ "$(sysctl -n kernel.apparmor_restrict_unprivileged_userns)" = 1 ]
