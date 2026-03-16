# Linux Port Architecture (Task B)

## Chosen decisions

- Stack choice: Hybrid architecture.
  - Zig hosts the Linux desktop shell and native UI integration points.
  - Rust remains the control plane and CLI/protocol execution engine.
- UI tree choice: GtkPaned-based shell topology.
  - Root split includes a persistent sidebar and content region.
  - Content region is a nested GtkPaned tree for terminal/browser compositions.
- Protocol support policy: v1 and v2 are both supported from day one of Linux host bring-up.
- Display backends: Wayland and X11 are both required targets.

## Subsystem boundaries

### Zig host subsystem

Responsibilities:

- Native Linux app lifecycle bootstrap
- GTK4/libadwaita/WebKit host composition and pane tree management
- Window/shell layout ownership (sidebar + nested GtkPaned topology)
- Thin FFI boundary that forwards lifecycle and command dispatch calls to Rust

Non-responsibilities:

- Business logic for command semantics
- Protocol policy evolution (outside host transport/surface wiring)
- Long-lived control-plane state machine decisions

### Rust control and CLI subsystem

Responsibilities:

- Canonical command model and policy engine
- v1 and v2 protocol handling
- CLI command surface and transport orchestration
- Validation, state transitions, and command execution invariants

Non-responsibilities:

- Linux desktop widget tree ownership
- Direct GTK/libadwaita/WebKit composition details

## Interop contract (initial placeholder)

Zig host expects a C ABI control-plane bridge with these calls:

- `cmux_control_init() -> int`
- `cmux_control_dispatch(const uint8_t* message_ptr, size_t message_len) -> int`
- `cmux_control_shutdown() -> void`

This remains intentionally thin to avoid locking to Rust internals while host scaffolding is built.

Current status:

- The Rust bridge now exports these symbols from `linux/rust/cmux-control` and Zig can link them with `-Denable-rust-ffi=true`.
- The Zig host probes the bridge during startup with a JSON `system.ping` request.

## Socket path policy

Canonical precedence (shared by server and CLI):

1. Explicit `--socket` argument
2. `CMUX_SOCKET`
3. `CMUX_SOCKET_PATH`
4. Mode default

Mode defaults:

- `runtime` mode: `$XDG_RUNTIME_DIR/cmux/cmux.sock` (fallback `/tmp/cmux.sock`)
- `debug` mode: `/tmp/cmux-debug.sock`

## Acceptance gates

Gate 1: scaffold integrity
- `linux/zig/build.zig` exists and builds default path without GTK dev packages.
- `linux/zig/src/main.zig` models layout intent and startup flow.
- `linux/zig/src/rust_control_ffi.zig` exposes init/dispatch/shutdown placeholders.

Gate 2: architecture alignment
- Hybrid split is documented and reflected in code boundaries.
- GtkPaned topology intent is represented in host constants/comments.
- v1+v2 policy and Wayland+X11 requirements are explicitly documented.

Gate 3: operational readiness
- README includes dependency expectations and build/run toggles.
- Feature toggles fail fast for invalid combinations (for example, libadwaita without GTK4).
- Rust bridge and Zig host link/run successfully in headless mode with `-Denable-rust-ffi=true`.

Gate 4: control-plane feature baseline
- v1 envelope + v2 JSON request handling over Unix socket is working.
- Core command families implemented: workspace/window/pane/surface/notification/app plus a browser P0 control surface.

## Packaging targets

Primary Linux deliverables:

- Native distro package targets: `.deb` and `.rpm`
- Portable desktop package: AppImage
- Fallback distribution artifact: tarball bundle with runtime notes

Packaging expectations:

- Runtime support for both Wayland and X11 sessions
- Deterministic build flags for GUI feature toggles
- Clear separation between host binary packaging and Rust control-plane artifact packaging
