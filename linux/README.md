# cmux-linux Foundation

This directory is the Linux port foundation for the cmux hybrid architecture:

- `rust/`: protocol, canonical core engine, control-plane adapters, host runtime, and CLI
- `zig/`: Linux host scaffold and FFI boundary to the Rust control plane

Current canonical runtime entrypoint:

- `cmux-linux` (Rust host runtime, package `cmux-host-linux`)

## Quickstart

Install GUI build dependencies (required for `--features gtk-ui`):

Ubuntu/Debian:

```bash
sudo apt update
sudo apt install -y libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev pkg-config build-essential
```

Homebrew (no sudo):

```bash
brew install gtk4 libadwaita vte3
eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
```

Verify pkg-config visibility:

```bash
pkg-config --modversion gtk4 libadwaita-1 vte-2.91-gtk4
```

Build Rust workspace:

```bash
cargo build --manifest-path linux/Cargo.toml
```

Run Linux host runtime in headless mode (starts control-plane socket server):

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --bin cmux-linux -- --headless
```

Run Linux host runtime with GTK shell (when built with GTK feature and system deps):

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --features gtk-ui --bin cmux-linux
```

Smoke-test GTK mode headlessly (useful on CI/headless hosts):

```bash
xvfb-run -a cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --features gtk-ui --bin cmux-linux -- --socket /tmp/cmux-gtk-smoke.sock
```

Build Zig host scaffold:

```bash
cd linux/zig
zig build
```

## Docs

- Linux architecture decisions: `docs/linux-port-architecture.md`
- Rust usage notes: `linux/rust/README.md`
- Zig host notes: `linux/zig/README.md`
