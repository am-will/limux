# cmux-core

`cmux-core` is the canonical Linux command/state engine for cmux.

It contains:
- in-memory workspace/window/pane/surface/browser state
- v2 command dispatch semantics
- command validation and behavior rules

Adapters use this crate:
- `cmux-control` for Unix socket server + FFI
- `cmux-host-linux` for host runtime integration

## Test

```bash
cargo test --manifest-path linux/Cargo.toml -p cmux-core
```
