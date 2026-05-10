# Plan: Limux Host Shortcut Remapping Config

**Generated**: 2026-03-24

## Overview
Implement config-backed shortcut remapping in the Linux host without coupling it to Ghostty config or session persistence. The canonical design is:

- Store user preferences in a dedicated Limux config file under `dirs::config_dir()`, not in Ghostty config and not in `session.json`
- Define one host-owned shortcut registry keyed by stable shortcut IDs
- Define one canonical metadata layer that maps each shortcut ID to its owner, runtime dispatch target, GTK accelerator usage, and user-visible label text
- Switch GTK application accelerators and capture-phase dispatch to that same registry in one implementation step so no broken intermediate state exists
- Treat empty bindings as explicitly unbound
- Update all visible shortcut hints in host UI surfaces, including `window.rs` and `pane.rs`

This plan keeps the shortcut feature first-class in the Linux host while avoiding a third shortcut path. `limux-core` command-palette shortcut hints remain out of scope for this first implementation and should be treated as a follow-up only if the host-side system stabilizes cleanly.

## Prerequisites
- Existing GTK4/libadwaita host build environment
- `dirs`, `serde`, and `serde_json` already available in the workspace
- Context7/GTK docs already checked for accelerator behavior and capture-phase shortcut handling

## Dependency Graph

```text
T1 ── T2 ── T3 ── T4 ──┬── T5 ── T6
                       └── T6
```

## Tasks

### T1: Inventory Current Host-Owned Shortcuts and Hint Surfaces
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`
- **description**: Audit every host-owned shortcut currently implemented through `app.set_accels_for_action(...)`, `register_actions()`, and `install_key_capture()`. Produce the frozen list of shortcut IDs, current default bindings, action owners, direct helper dispatch targets, GTK-global actions, capture-only actions, and all user-visible hint surfaces that currently embed hardcoded shortcut text. Explicitly mark terminal-owned combos that must always pass through to Ghostty and are out of scope for interception.
- **validation**: The implementation has a complete checklist covering all current host shortcut paths and every visible tooltip/label surface that would drift if left hardcoded.
- **status**: Completed
- **log**: `reason_not_testable`: inventory-only task. Verified by direct code inspection. Current GTK-global actions are only `win.new-workspace`, `win.close-workspace`, `win.toggle-sidebar`, `win.next-workspace`, and `win.prev-workspace` in `rust/limux-host-linux/src/main.rs:103-107`, with matching `gio::SimpleAction` wiring in `rust/limux-host-linux/src/window.rs:827-849`. Capture-only host shortcuts are implemented in `rust/limux-host-linux/src/window.rs:864-980`: `new_workspace`, `close_workspace`, `cycle_tab_prev`, `cycle_tab_next`, `split_down`, `new_terminal`, `split_right`, `close_focused_pane`, `toggle_sidebar`, `next_workspace`, `prev_workspace`, `focus_left`, `focus_right`, `focus_up`, `focus_down`, and `activate_workspace_1` through `activate_workspace_9_or_last`. Gotchas for follow-up tasks: `Ctrl+T` and `Ctrl+Shift+T` both dispatch to `add_tab_to_focused_pane(false)` in `rust/limux-host-linux/src/window.rs:890-913`; only five actions currently exist as `gio::SimpleAction`s; pane action buttons are wired independently in `rust/limux-host-linux/src/pane.rs:244-278`; and Ghostty terminal input remains the passthrough owner for unmapped keys via `ghostty_surface_key(...)` in `rust/limux-host-linux/src/terminal.rs:566-610`. UI surfaces with hardcoded shortcut text are the sidebar collapse and expand tooltips in `rust/limux-host-linux/src/window.rs:623` and `rust/limux-host-linux/src/window.rs:683`. Pane buttons in `rust/limux-host-linux/src/pane.rs:190-194` expose action tooltips without shortcut text today and will need registry-backed labels once remapping exists.
- **files edited/created**: `shortcut-remap-plan.md`

### T2: Define Canonical Shortcut Metadata and Dispatch Layer
- **depends_on**: [T1]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs` (new), `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/pane.rs`
- **description**: Create the first-class host shortcut definition layer. Each definition should capture stable shortcut ID, default binding, runtime owner, whether it registers a GTK accelerator, the dispatch target used by capture-phase routing, and the human-readable label/tooltip name. This is the canonical registry that both `register_actions()` and `install_key_capture()` will consume. The layer should also decide which actions remain direct helper dispatches and which are backed by `gio::SimpleAction`.
- **validation**: There is one authoritative metadata table for host shortcuts, and every current shortcut from T1 maps to exactly one runtime dispatch target and one visibility policy.
- **status**: Completed
- **log**: Verified existing branch state rather than re-implementing. `rust/limux-host-linux/src/shortcut_config.rs` already provides the canonical host shortcut metadata layer with stable IDs, config keys, action names, labels, GTK registration policy, and runtime command targets. Validation command: `cargo test -p limux-host-linux shortcut_config::tests -- --nocapture` passed, confirming the 25-definition table, uniqueness invariants, the GTK accelerator subset, and canonical runtime command mapping. Non-blocking note for follow-up: `find_by_action_name` is currently unused and triggers a `dead_code` warning.
- **files edited/created**: `rust/limux-host-linux/src/shortcut_config.rs`, `shortcut-remap-plan.md`

### T3: Implement Config Schema, Path Resolution, and Validation Rules
- **depends_on**: [T2]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs` (new), `rust/limux-host-linux/Cargo.toml`
- **description**: Implement the dedicated host-side shortcut config loader and merger. The config file should live at `dirs::config_dir()/limux/config.json` with deterministic overrides for tests. The schema should support omitted values for defaults and empty-string or `null` values for explicit unbinding. Make the contract explicit for these cases: `config_dir()` returning `None`, unreadable files, invalid JSON, unknown shortcut IDs, duplicate active bindings, malformed bindings, and any binding that cannot be represented consistently across GTK accelerator registration and capture-phase normalization. Use clear logging plus fallback-to-default behavior for runtime file/load failures, and fail validation for ambiguous active duplicate bindings.
- **validation**: The loader resolves the expected config path, merges overrides over defaults, preserves explicit unbinds, warns or errors exactly as specified for invalid inputs, and always returns a deterministic effective registry.
- **status**: Completed
- **log**: Verified existing branch state rather than re-implementing. `rust/limux-host-linux/src/shortcut_config.rs` already resolves `dirs::config_dir()/limux/config.json`, loads JSON overrides under the top-level `shortcuts` key, supports explicit unbinding via `null` or empty string, warns on unknown IDs, falls back to defaults on missing files and invalid JSON, and rejects duplicate active bindings before runtime use. Validation command: `cargo test -p limux-host-linux shortcut_config::tests -- --nocapture` passed.
- **files edited/created**: `rust/limux-host-linux/src/shortcut_config.rs`, `shortcut-remap-plan.md`

### T4: Add Unit Tests for Config Loading and Normalization
- **depends_on**: [T3]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`
- **description**: Add focused unit tests for config path derivation, default loading when no file exists, override application, explicit unbinding, invalid JSON fallback, unknown shortcut IDs, duplicate-binding rejection, malformed accelerator rejection, and normalization round-trips between stored values and runtime representations. Keep these tests pure and tempdir-driven so they do not depend on GTK startup.
- **validation**: `cargo test -p limux-host-linux` covers the config contract and fails if loader behavior regresses on any supported edge case.
- **status**: Completed
- **log**: Verified existing branch state rather than re-implementing. The targeted suite in `rust/limux-host-linux/src/shortcut_config.rs` already covers path derivation, normalized shortcut round-trips, override application, explicit unbinding, unknown ID warnings, duplicate-binding rejection, invalid JSON fallback, missing-file defaults, GTK accelerator exposure for unbound actions, and runtime combo-to-command routing. Validation command: `cargo test -p limux-host-linux shortcut_config::tests -- --nocapture` passed with 12 tests.
- **files edited/created**: `rust/limux-host-linux/src/shortcut_config.rs`, `shortcut-remap-plan.md`

### T5: Switch GTK Accelerators and Capture-Phase Dispatch to the Same Registry
- **depends_on**: [T4]
- **location**: `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Replace the current hardcoded startup accelerators and the hardcoded capture-phase `match` with one registry-driven implementation in a single change. Startup should load the effective shortcut registry once, apply GTK accelerators from that registry, and ensure explicit unbinds clear accelerators. `install_key_capture()` should normalize incoming key events, resolve them through the same registry, and dispatch the mapped host action. Preserve passthrough to Ghostty for unmapped events. Do not leave any overlapping hardcoded capture bindings behind, because that would create dual active routes during remapped states.
- **validation**: Default bindings preserve current behavior, remapped bindings trigger the correct host actions, old bindings stop working once remapped, explicitly unbound actions stop intercepting input, and unmapped keys continue through to terminal surfaces.
- **status**: Completed
- **log**: RED phase added two new regression tests in `rust/limux-host-linux/src/shortcut_config.rs` for the runtime integration seam: `resolved_shortcuts_expose_registered_gtk_accels_and_clear_unbound_actions` and `resolved_shortcuts_route_runtime_combos_to_canonical_commands`. Initial validation failed with missing helper methods on `ResolvedShortcutConfig`. GREEN changes then made the shortcut registry authoritative at runtime: `rust/limux-host-linux/src/main.rs` now loads `shortcut_config::load_shortcuts()`, prints warnings once at startup, applies GTK accelerator bindings from `ResolvedShortcutConfig::gtk_accel_entries()`, and passes the resolved registry into `window::build_window(...)`. `rust/limux-host-linux/src/window.rs` now stores the resolved registry in `AppState`, registers all window actions from shortcut metadata, resolves capture-phase key events through `NormalizedShortcut::from_gdk_key(...)`, and dispatches canonical `ShortcutCommand` values through a single `dispatch_shortcut_command(...)` helper. The old hardcoded key-combo `match` was removed, so GTK accelerator registration and capture dispatch now derive from the same registry instead of separate hardcoded tables.
- **files edited/created**: `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`

### T6: Update All Host UI Shortcut Hints and Add Regression Coverage
- **depends_on**: [T5]
- **location**: `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`, focused helper tests where appropriate
- **description**: Remove hardcoded visible shortcut strings and derive tooltip/label text from the same effective registry used at runtime. This includes sidebar toggle strings in `window.rs` and pane action tooltips currently constructed through `icon_button()` in `pane.rs`. Add regression tests for tooltip rendering and runtime mapping helpers, including the highest-risk behavior: remaps, explicit unbinds, malformed config fallback, duplicate rejection, unknown IDs, normalization round-trips, and proof that old bindings are no longer intercepted once remapped or unbound.
- **validation**: Tooltips and labels reflect remapped shortcuts, unbound actions omit shortcut suffixes, and tests fail if a hardcoded host shortcut hint or stale binding path is reintroduced.
- **status**: Completed
- **log**: RED phase added pure tooltip-contract tests in `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`, and `rust/limux-host-linux/src/pane.rs`, then ran `cargo test -p limux-host-linux tooltip -- --nocapture`, which failed because the display/tooltip helpers and call sites did not exist. GREEN changes added one shared display path in `rust/limux-host-linux/src/shortcut_config.rs` (`to_display_label`, `display_label_for_id`, and `tooltip_text`), replaced the hardcoded sidebar toggle tooltip strings in `rust/limux-host-linux/src/window.rs` with `sidebar_toggle_tooltip(...)`, and threaded the resolved shortcut registry into `rust/limux-host-linux/src/pane.rs` so pane action buttons derive tooltip text from the same registry while unbound actions omit shortcut suffixes. Also removed the unused `find_by_action_name` helper to keep the branch warning-free. GREEN commands: `cargo test -p limux-host-linux tooltip -- --nocapture`, `cargo test -p limux-host-linux`, and `cargo build -p limux-host-linux --features webkit` all passed.
- **files edited/created**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`, `shortcut-remap-plan.md`

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1 | Immediately |
| 2 | T2 | T1 complete |
| 3 | T3 | T2 complete |
| 4 | T4 | T3 complete |
| 5 | T5 | T4 complete |
| 6 | T6 | T5 complete |

## Testing Strategy
- Run `cargo test -p limux-host-linux`
- Run `cargo build -p limux-host-linux --features webkit`
- Manually validate these runtime cases:
  - No config file: default shortcuts still work
  - Override file with one remap: new binding works and old binding no longer does
  - Override file with one explicit unbind: host no longer intercepts that combo and Ghostty receives it
  - Invalid JSON or unknown IDs: host logs the failure path and falls back to defaults deterministically
  - Duplicate active bindings: config is rejected according to the chosen validation contract, with no ambiguous runtime interception
- Launch the host for manual verification with:

```bash
LD_LIBRARY_PATH="/home/willr/Applications/cmux-linux/cmux/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
cargo run -p limux-host-linux --features webkit --bin limux
```

## Risks & Mitigations
- GTK accelerator strings and capture-phase event matching use different formats.
  - Mitigation: keep one logical shortcut model and maintain two explicit renderers/parsers, one for GTK accelerator strings and one for normalized runtime matching.
- Startup config load failure could silently leave the app in a confusing state.
  - Mitigation: log parse and validation failures clearly, then fall back to code defaults.
- Duplicate bindings could create nondeterministic action routing.
  - Mitigation: reject duplicate active bindings during config validation before they reach registration or dispatch.
- Session persistence and preferences could be accidentally mixed.
  - Mitigation: keep shortcut config in a separate module and file under `config_dir`, with no additions to `AppSessionState`.
- Static tooltip strings can drift from runtime behavior.
  - Mitigation: derive visible shortcut hints from the same registry used for accelerator registration and capture-phase dispatch, including pane action buttons.
- `limux-core` command-palette shortcut hints currently use a separate model.
  - Mitigation: explicitly keep that out of scope for the first host implementation and do not claim single-source-of-truth beyond the Linux host until a later extraction is done.
