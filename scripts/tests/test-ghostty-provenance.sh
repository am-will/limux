#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
FIXTURE="$TMP_DIR/checkout"
mkdir -p "$FIXTURE/scripts/tests" "$FIXTURE/ghostty" "$TMP_DIR/bin"
cp "$ROOT_DIR/scripts/"{build-ghostty,check-ghostty,check,xvfb-smoke-test}.sh "$FIXTURE/scripts/"
cp "$ROOT_DIR/scripts/tests/"{test-terminal-cwd,test-tab-rename,test-window-activate}.sh "$FIXTURE/scripts/tests/"
cat > "$TMP_DIR/bin/cargo" <<'STUB'
#!/usr/bin/env bash
printf 'cargo\n' >> "$TEST_COMMAND_LOG"
exit 37
STUB
cat > "$TMP_DIR/bin/dbus-run-session" <<'STUB'
#!/usr/bin/env bash
printf 'dbus\n' >> "$TEST_COMMAND_LOG"
exit 37
STUB
# Simulate compiler output, while exercising the real build wrapper and checks.
cat > "$TMP_DIR/bin/zig" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1" = version ]; then printf '%s\n' "${TEST_ZIG_VERSION:-0.16.0}"; exit; fi
[ "$1" = build ]
if [ -n "${TEST_ZIG_ATTEMPT_FILE:-}" ]; then
  count=0
  if [ -f "$TEST_ZIG_ATTEMPT_FILE" ]; then
    count="$(cat "$TEST_ZIG_ATTEMPT_FILE")"
  fi
  count=$((count + 1))
  printf '%s\n' "$count" > "$TEST_ZIG_ATTEMPT_FILE"
  if [ "$count" -le "${TEST_ZIG_TRANSIENT_FETCH_FAILS:-0}" ]; then
    if [ $((count % 2)) -eq 1 ]; then
      echo "ghostty/pkg/wuffs/build.zig.zon:21:20: error: HTTP response read failure: ConnectionResetByPeer" >&2
    else
      echo "ghostty/build.zig.zon:120:20: error: HTTP request failed: WriteFailed" >&2
    fi
    exit 1
  fi
fi
if [ "${TEST_ZIG_FAIL:-0}" = 1 ]; then exit 42; fi
if [ "${TEST_ZIG_NO_OUTPUT:-0}" = 1 ]; then exit; fi
mkdir -p zig-out/lib
if [[ " $* " == *' -Dstrip=false '* ]] && [ "${TEST_ZIG_IGNORE_DEBUG:-0}" = 0 ]; then
  printf 'void ghostty_debug_fixture(void) {}\n' | cc -g -shared -x c - -o zig-out/lib/libghostty-internal.so
else
  printf 'library from %s\n' "$(git rev-parse HEAD)" > zig-out/lib/libghostty-internal.so
fi
if [ "${TEST_ZIG_DIRTY:-0}" = 1 ]; then printf 'changed during build\n' >> build.zig; fi
STUB
chmod +x "$TMP_DIR/bin/"*
export PATH="$TMP_DIR/bin:$PATH" ZIG="$TMP_DIR/bin/zig" TEST_COMMAND_LOG="$TMP_DIR/commands"
LIBRARY="$FIXTURE/ghostty/zig-out/lib/libghostty-internal.so"
PROVENANCE="$FIXTURE/ghostty/zig-out/lib/limux-ghostty-build"

reject() {
  local expected="$1"
  shift
  if "$@" >"$TMP_DIR/output" 2>&1; then
    echo "FAIL: accepted $expected" >&2
    exit 1
  fi
  if ! grep -Fq "$expected" "$TMP_DIR/output" || ! grep -Fq './scripts/build-ghostty.sh' "$TMP_DIR/output"; then
    cat "$TMP_DIR/output" >&2
    echo "FAIL: missing actionable diagnostic for $expected" >&2
    exit 1
  fi
}

# An empty submodule directory must not inherit the parent repository's HEAD.
git init -q "$FIXTURE"
printf 'fixture\n' > "$FIXTURE/ghostty/build.zig"
reject 'not an initialized Git checkout' "$FIXTURE/scripts/check-ghostty.sh"
git init -q "$FIXTURE/ghostty"
git -C "$FIXTURE/ghostty" config user.name 'Provenance test'
git -C "$FIXTURE/ghostty" config user.email 'test@example.invalid'
git -C "$FIXTURE/ghostty" config commit.gpgsign false
printf 'zig-out/\n' > "$FIXTURE/ghostty/.gitignore"
git -C "$FIXTURE/ghostty" add build.zig .gitignore
git -C "$FIXTURE/ghostty" commit -qm fixture
reject 'shared library is missing' "$FIXTURE/scripts/check-ghostty.sh"

mkdir -p "$(dirname "$LIBRARY")"
printf 'unrecorded library\n' > "$LIBRARY"
for entrypoint in check.sh xvfb-smoke-test.sh tests/test-terminal-cwd.sh tests/test-tab-rename.sh tests/test-window-activate.sh; do
  reject 'provenance is missing' "$FIXTURE/scripts/$entrypoint"
done
if [ -s "$TEST_COMMAND_LOG" ]; then
  echo 'FAIL: ran Cargo or D-Bus against an unrecorded library' >&2
  exit 1
fi

"$FIXTURE/scripts/build-ghostty.sh" >/dev/null
"$FIXTURE/scripts/check-ghostty.sh" >/dev/null
# A matching artifact is allowed through the real quality entrypoint to Cargo.
result=0
"$FIXTURE/scripts/check.sh" >"$TMP_DIR/output" 2>&1 || result=$?
[ "$result" -eq 37 ] && [ "$(cat "$TEST_COMMAND_LOG")" = cargo ]

printf 'next revision\n' >> "$FIXTURE/ghostty/build.zig"
git -C "$FIXTURE/ghostty" commit -qam next
reject 'revision mismatch' "$FIXTURE/scripts/check-ghostty.sh"
"$FIXTURE/scripts/build-ghostty.sh" >/dev/null
cp "$LIBRARY" "$TMP_DIR/library"
printf 'replaced\n' >> "$LIBRARY"
reject 'checksum changed' "$FIXTURE/scripts/check-ghostty.sh"
cp "$TMP_DIR/library" "$LIBRARY"
"$FIXTURE/scripts/check-ghostty.sh" >/dev/null

# Identical bytes at an external target are still a changed library path.
mv "$LIBRARY" "$TMP_DIR/external.so"
ln -s "$TMP_DIR/external.so" "$LIBRARY"
reject 'library target changed' "$FIXTURE/scripts/check-ghostty.sh"
"$FIXTURE/scripts/build-ghostty.sh" >/dev/null
[ ! -L "$LIBRARY" ]
cmp "$TMP_DIR/library" "$TMP_DIR/external.so"

if TEST_ZIG_FAIL=1 "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted a failed Zig build' >&2
  exit 1
fi
[ ! -e "$PROVENANCE" ]
reject 'shared library is missing' "$FIXTURE/scripts/check-ghostty.sh"
reject 'did not produce a local Ghostty shared library' env TEST_ZIG_NO_OUTPUT=1 "$FIXTURE/scripts/build-ghostty.sh"
[ ! -e "$PROVENANCE" ]
reject 'source has local changes' env TEST_ZIG_DIRTY=1 "$FIXTURE/scripts/build-ghostty.sh"
[ ! -e "$PROVENANCE" ]
git -C "$FIXTURE/ghostty" restore build.zig
reject 'requires Zig 0.16.0' env TEST_ZIG_VERSION=0.15.2 "$FIXTURE/scripts/build-ghostty.sh"

if LIMUX_GHOSTTY_DEBUG_SYMBOLS=invalid "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted invalid debug-symbol mode' >&2
  exit 1
fi
grep -Fq 'LIMUX_GHOSTTY_DEBUG_SYMBOLS must be 0 or 1' "$TMP_DIR/output"
LIMUX_GHOSTTY_DEBUG_SYMBOLS=1 "$FIXTURE/scripts/build-ghostty.sh" >/dev/null
"$FIXTURE/scripts/check-ghostty.sh" >/dev/null
reject 'did not retain requested debug symbols' env LIMUX_GHOSTTY_DEBUG_SYMBOLS=1 TEST_ZIG_IGNORE_DEBUG=1 "$FIXTURE/scripts/build-ghostty.sh"
[ ! -e "$PROVENANCE" ]

# AUR builds use a symlink to a separate Git checkout as the source submodule.
mv "$FIXTURE/ghostty" "$TMP_DIR/aur-ghostty"
ln -s "$TMP_DIR/aur-ghostty" "$FIXTURE/ghostty"
"$FIXTURE/scripts/build-ghostty.sh" >/dev/null
"$FIXTURE/scripts/check-ghostty.sh" >/dev/null

# Transient Zig dependency fetch failures retry and succeed once downloads recover.
ATTEMPT_FILE="$TMP_DIR/zig-attempts"
rm -f "$ATTEMPT_FILE"
LIMUX_GHOSTTY_FETCH_ATTEMPTS=4 \
  LIMUX_GHOSTTY_FETCH_RETRY_DELAY_SECS=0 \
  TEST_ZIG_ATTEMPT_FILE="$ATTEMPT_FILE" \
  TEST_ZIG_TRANSIENT_FETCH_FAILS=2 \
  "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1
[ "$(cat "$ATTEMPT_FILE")" -eq 3 ]
grep -Fq 'retrying in 0s' "$TMP_DIR/output"
"$FIXTURE/scripts/check-ghostty.sh" >/dev/null

# Exhausting configured fetch attempts fails without recording provenance.
rm -f "$ATTEMPT_FILE"
if LIMUX_GHOSTTY_FETCH_ATTEMPTS=2 \
  LIMUX_GHOSTTY_FETCH_RETRY_DELAY_SECS=0 \
  TEST_ZIG_ATTEMPT_FILE="$ATTEMPT_FILE" \
  TEST_ZIG_TRANSIENT_FETCH_FAILS=3 \
  "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted exhausted transient Zig fetch retries' >&2
  exit 1
fi
[ "$(cat "$ATTEMPT_FILE")" -eq 2 ]
[ ! -e "$PROVENANCE" ]

# Non-fetch Zig build failures must not retry.
rm -f "$ATTEMPT_FILE"
if LIMUX_GHOSTTY_FETCH_ATTEMPTS=4 \
  LIMUX_GHOSTTY_FETCH_RETRY_DELAY_SECS=0 \
  TEST_ZIG_ATTEMPT_FILE="$ATTEMPT_FILE" \
  TEST_ZIG_FAIL=1 \
  "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted non-fetch Zig failure' >&2
  exit 1
fi
[ "$(cat "$ATTEMPT_FILE")" -eq 1 ]

# Reject invalid retry configuration values.
if LIMUX_GHOSTTY_FETCH_ATTEMPTS=0 "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted zero fetch attempts' >&2
  exit 1
fi
grep -Fq 'LIMUX_GHOSTTY_FETCH_ATTEMPTS must be a positive integer' "$TMP_DIR/output"
if LIMUX_GHOSTTY_FETCH_RETRY_DELAY_SECS=bad "$FIXTURE/scripts/build-ghostty.sh" >"$TMP_DIR/output" 2>&1; then
  echo 'FAIL: accepted invalid fetch retry delay' >&2
  exit 1
fi
grep -Fq 'LIMUX_GHOSTTY_FETCH_RETRY_DELAY_SECS must be a non-negative integer' "$TMP_DIR/output"

# Restore valid artifact before testing dirty submodule rejection.
"$FIXTURE/scripts/build-ghostty.sh" >/dev/null
printf 'uncommitted\n' >> "$FIXTURE/ghostty/build.zig"
reject 'source has local changes' "$FIXTURE/scripts/check-ghostty.sh"

echo 'Ghostty provenance checks: OK'
