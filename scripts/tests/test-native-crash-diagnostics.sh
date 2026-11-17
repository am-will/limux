#!/usr/bin/env bash
# --kernel validates the CI core setup; --gdb validates collection without changing the host.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
MODE="${1:---gdb}"
case "$MODE" in --kernel|--gdb) ;; *) echo "Expected --kernel or --gdb"; exit 2 ;; esac
if [ "$MODE" = --kernel ]; then
  capture_dir="${RUNNER_TEMP:?}/limux-native-crashes"
  TEST_DIR="$(mktemp -d "$capture_dir/self-test-XXXXXX")"
else
  TEST_DIR="$(mktemp -d -t limux-native-crash-test-XXXXXX)"
fi
echo "Native crash test artifacts: $TEST_DIR"
mkdir "$TEST_DIR/cores"
fixture_dir="$TEST_DIR/target/debug/build/limux-host-linux-0123456789abcdef/out"
mkdir -p "$fixture_dir"
fixture="$fixture_dir/limux-abort-fixture-0123456789abcdef"
cat > "$TEST_DIR/abort-fixture.c" <<'C'
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <unistd.h>
static atomic_int ready;
static void *waiting_worker(void *unused) {
    (void)unused;
    atomic_store(&ready, 1);
    for (;;) pause();
    return NULL;
}
static void crash_here(void) {
    const char *message = "truncated is application data";
    (void)message;
    abort();
}
int main(void) {
    pthread_t worker;
    if (pthread_create(&worker, NULL, waiting_worker, NULL)) return 2;
    while (!atomic_load(&ready)) usleep(1000);
    crash_here();
}
C
cc -g -O0 -pthread "$TEST_DIR/abort-fixture.c" -o "$fixture"
if [ "$MODE" = --kernel ]; then
  result=0
  uv run "$ROOT_DIR/scripts/ci-native-crashes.py" run -- "$fixture" || result=$?
  [ "$result" -eq 134 ] || { echo "FAIL: expected SIGABRT exit 134, got $result"; exit 1; }
  for core in "$capture_dir"/cores/core.*; do
    case "$core" in *limux-abort-fixture-0123456789abcdef) mv "$core" "$TEST_DIR/cores/" ;; esac
  done
else
  encoded="${fixture//\//!}"
  timeout 30 gdb -nx -nh -batch -iex 'set auto-load off' -iex 'set debuginfod enabled off' \
    -ex run -ex "generate-core-file $TEST_DIR/cores/core.0.0.$encoded" \
    --args "$fixture" > "$TEST_DIR/generation.txt" 2>&1
fi
shopt -s nullglob
cores=("$TEST_DIR"/cores/core.*)
[ "${#cores[@]}" -eq 1 ] || { echo "FAIL: expected one native core, found ${#cores[@]}"; exit 1; }
uv run "$ROOT_DIR/scripts/ci-native-crashes.py" --directory "$TEST_DIR" collect
grep -q 'crash_here .*abort-fixture.c:' "$TEST_DIR"/*.backtrace.txt
grep -q 'waiting_worker .*abort-fixture.c:' "$TEST_DIR"/*.backtrace.txt
grep -q 'SIGABRT' "$TEST_DIR"/*.backtrace.txt
grep -q 'message = .*truncated is application data' "$TEST_DIR"/*.backtrace.txt
tar -tzf "$TEST_DIR/objects.tar.gz" | grep '/limux-abort-fixture-0123456789abcdef$' >/dev/null

# Removing core data must fail collection, even when GDB itself returns success.
mkdir -p "$TEST_DIR/partial-core/cores"
partial_core="$TEST_DIR/partial-core/cores/$(basename "${cores[0]}")"
cp "${cores[0]}" "$partial_core"
truncate -s "$(( $(stat -c %s "$partial_core") / 2 ))" "$partial_core"
result=0
uv run "$ROOT_DIR/scripts/ci-native-crashes.py" --directory "$TEST_DIR/partial-core" collect \
  > "$TEST_DIR/partial-core/collector.log" 2>&1 || result=$?
[ "$result" -eq 1 ] || { echo "FAIL: partial core was not a collection failure"; exit 1; }
jq -e '.errors[] | contains("GDB reported incomplete or mismatched core data")' \
  "$TEST_DIR/partial-core/manifest.json" >/dev/null

# A collection error must remain visible without losing the raw core.
mkdir -p "$TEST_DIR/missing-executable/cores"
cp "${cores[0]}" "$TEST_DIR/missing-executable/cores/core.0.0.!missing-limux-crash-fixture"
result=0
uv run "$ROOT_DIR/scripts/ci-native-crashes.py" --directory "$TEST_DIR/missing-executable" collect \
  > "$TEST_DIR/missing-executable/collector.log" 2>&1 || result=$?
[ "$result" -eq 1 ] || { echo "FAIL: missing executable was not a collection failure"; exit 1; }
jq -e '.errors[] | contains("matching executable is missing")' \
  "$TEST_DIR/missing-executable/manifest.json" >/dev/null
cmp "${cores[0]}" "$TEST_DIR/missing-executable/cores/core.0.0.!missing-limux-crash-fixture"

# Exercise exec's status propagation without requiring local kernel configuration.
jq -n --arg pattern "$(cat /proc/sys/kernel/core_pattern)" '{core_pattern:$pattern}' > "$TEST_DIR/capture.json"
result=0
uv run "$ROOT_DIR/scripts/ci-native-crashes.py" --directory "$TEST_DIR" run -- sh -c 'exit 17' || result=$?
[ "$result" -eq 17 ] || { echo "FAIL: wrapped command status changed to $result"; exit 1; }
echo 'PASS: captured SIGABRT with symbolized crashing and worker threads plus matching objects'
