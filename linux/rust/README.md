# cmux Linux Rust Foundation

This directory contains the Linux Rust control-plane workspace.

## Workspace Crates

- `cmux-protocol`: Protocol types and parsing helpers.
- `cmux-core`: Canonical command/state engine shared by adapters.
- `cmux-control`: Adapter crate (Unix socket server + FFI bridge over `cmux-core`).
- `cmux-cli`: Command-line client for sending one v2 JSON request.
- `cmux-host-linux`: Linux host runtime binary (`cmux-linux`).

`cmux-control` also exports a C ABI bridge for the Zig host:

- `cmux_control_init() -> int`
- `cmux_control_dispatch(const uint8_t* message_ptr, size_t message_len) -> int`
- `cmux_control_shutdown() -> void`

## Build

```bash
cargo build --manifest-path linux/Cargo.toml
```

## GTK Feature Prereqs

GUI builds require these pkg-config entries:

- `gtk4`
- `libadwaita-1`
- `vte-2.91-gtk4`

Install on Ubuntu/Debian:

```bash
sudo apt update
sudo apt install -y libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev pkg-config build-essential
```

Or with Homebrew:

```bash
brew install gtk4 libadwaita vte3
eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
```

Verify:

```bash
pkg-config --modversion gtk4 libadwaita-1 vte-2.91-gtk4
```

## Run Linux Host Runtime

Headless mode (always available, no GTK system packages required):

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --bin cmux-linux -- --headless
```

GTK shell mode (requires `--features gtk-ui` and GTK pkg-config dependencies):

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --features gtk-ui --bin cmux-linux
```

## Run Server

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-control --bin cmux-control-server
```

Run host runtime with explicit socket path:

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-host-linux --bin cmux-linux -- --headless --socket /tmp/cmux-linux.sock
```

## Send a Request

```bash
cargo run --manifest-path linux/Cargo.toml -p cmux-cli -- \
  --request '{"id":"1","method":"system.ping","params":{}}'
```

## Socket path policy

Both server and CLI share one canonical resolution policy:

1. Explicit `--socket` argument
2. `CMUX_SOCKET` environment variable
3. `CMUX_SOCKET_PATH` environment variable
4. Mode default:
   - `runtime` mode (default): `$XDG_RUNTIME_DIR/cmux/cmux.sock`, fallback `/tmp/cmux.sock`
   - `debug` mode: `/tmp/cmux-debug.sock`

Select mode with `--socket-mode runtime|debug`.

## Test

```bash
cargo test --manifest-path linux/Cargo.toml
```

Also run the host-runtime matrix with GTK feature enabled:

```bash
cargo test --manifest-path linux/Cargo.toml -p cmux-host-linux --features gtk-ui
```
