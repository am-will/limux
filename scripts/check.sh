#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GHOSTTY_LIB_DIR="$ROOT_DIR/ghostty/zig-out/lib"
"$ROOT_DIR/scripts/check-ghostty.sh"

export LD_LIBRARY_PATH="$GHOSTTY_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$ROOT_DIR"

cargo fmt --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace -- --test-threads=1
./scripts/tests/test-release-version.sh
./scripts/tests/test-package-entrypoint.sh
./scripts/tests/test-user-prefix-install.sh
./scripts/tests/test-package-svg-loader.sh
./scripts/tests/test-aur-source-package.sh
./scripts/tests/test-smoke-child-count.sh
./scripts/tests/test-ghostty-provenance.sh
