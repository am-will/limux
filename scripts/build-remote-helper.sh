#!/usr/bin/env bash
# Static CLI only: no GTK/Ghostty or remote downloads. Install the Rust musl
# target and cross linker first if absent. Override TARGET for an ARM64 host.
set -euo pipefail
cd "$(dirname "$0")/.."
target="${TARGET:-x86_64-unknown-linux-musl}"
case "$target" in x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;; *) echo 'Use a supported Linux musl target' >&2; exit 2;; esac
cargo build --locked --release -p limux-cli --target "$target"
printf 'Select this trusted helper in Connect via SSH → Install agent helper:\n%s/target/%s/release/limux-cli\n' "$PWD" "$target"
