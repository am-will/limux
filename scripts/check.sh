#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR"

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
