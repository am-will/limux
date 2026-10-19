#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

# Exercise the production assertion without building or installing packages.
# shellcheck disable=SC1090
source <(sed -n '/^assert_cli_entrypoint() {$/,/^}$/p' "$ROOT_DIR/scripts/package.sh")

large_help() {
    printf 'limux CLI\n'
    # More than a pipe buffer: an early-closing grep must not kill the producer.
    printf '%1048576s\n' ''
}

failed_help() {
    printf 'limux CLI\n'
    return 42
}

host_help() {
    printf 'GApplication options\n'
}

if ! (assert_cli_entrypoint large_help "large CLI help"); then
    echo "FAIL: rejected successful CLI help larger than a pipe buffer" >&2
    exit 1
fi

if (assert_cli_entrypoint failed_help "failed CLI help") >/dev/null 2>&1; then
    echo "FAIL: accepted a failing help command because its output matched" >&2
    exit 1
fi

if (assert_cli_entrypoint host_help "host help") >/dev/null 2>&1; then
    echo "FAIL: accepted a non-CLI entrypoint" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d -t limux-flatpak-entrypoint-XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT
FLATPAK_BRANCH_DIR="$TMP_DIR/app/dev.limux.linux/x86_64/master"
mkdir -p "$FLATPAK_BRANCH_DIR/commit/files/libexec/limux" "$TMP_DIR/bin"
cp "$ROOT_DIR/flatpak/limux-host-cli" "$FLATPAK_BRANCH_DIR/commit/files/libexec/limux/limux-host-cli"
ln -s commit "$FLATPAK_BRANCH_DIR/active"
cat >"$TMP_DIR/bin/flatpak" <<'SH'
#!/bin/sh
printf '%s\0' "$@" >"$LIMUX_TEST_ARGUMENTS"
cat >"$LIMUX_TEST_STDIN"
SH
chmod 755 "$TMP_DIR/bin/flatpak"
printf '%s\0' run --arch=x86_64 --branch=master --command=limux dev.limux.linux \
  --json hooks claude cleanup 'a b' "it's literal" "\$HOME" $'line\nbreak' >"$TMP_DIR/expected-arguments"
printf '%s' '{"session_id":"synthetic"}' >"$TMP_DIR/payload"
PATH="$TMP_DIR/bin:$PATH" LIMUX_TEST_ARGUMENTS="$TMP_DIR/arguments" LIMUX_TEST_STDIN="$TMP_DIR/stdin" \
  "$FLATPAK_BRANCH_DIR/active/files/libexec/limux/limux-host-cli" \
  --json hooks claude cleanup 'a b' "it's literal" "\$HOME" $'line\nbreak' <"$TMP_DIR/payload"
cmp "$TMP_DIR/expected-arguments" "$TMP_DIR/arguments"
cmp "$TMP_DIR/payload" "$TMP_DIR/stdin"

echo "package CLI entrypoint validation: OK"
