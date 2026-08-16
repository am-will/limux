#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GHOSTTY_LIB_DIR="$ROOT_DIR/ghostty/zig-out/lib"
GHOSTTY_SO="$GHOSTTY_LIB_DIR/libghostty-internal.so"

if [ ! -f "$GHOSTTY_SO" ]; then
  echo "ERROR: libghostty-internal.so not found at $GHOSTTY_SO" >&2
  echo "Build it with Zig 0.16: (cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline)" >&2
  exit 1
fi

export LD_LIBRARY_PATH="$GHOSTTY_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$ROOT_DIR"

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
./scripts/tests/test-release-version.sh
./scripts/tests/test-package-svg-loader.sh
./scripts/tests/test-aur-source-package.sh
