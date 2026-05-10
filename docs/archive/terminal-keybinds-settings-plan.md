# Plan: Terminal Keybinds Settings Menu

**Generated**: 2026-03-24
**Base Commit**: `5b334b6c` (`host: finalize configurable shortcut registry`)

## Overview
Add a `Keybinds` entry to the existing terminal right-click context menu and route it to a dedicated shortcut-editor popover. The editor should list every host-owned shortcut from the canonical registry, show the current active binding plus the default binding, let the user click into a capture field that listens for the next combo, and persist valid changes back to `~/.config/limux/config.json`.

The implementation should stay inside the existing Linux host shortcut system rather than inventing a second settings layer. That means the same canonical definitions in `shortcut_config.rs` should drive:

- the keybinds editor rows
- runtime shortcut interception
- GTK application accelerators
- visible tooltip text across the window and pane chrome
- config persistence and validation

To keep the host maintainable and reduce merge conflicts during parallel execution, the editor UI and capture-state logic should live in a dedicated module such as `rust/limux-host-linux/src/keybind_editor.rs`, with `window.rs` owning only the open/apply integration points.

Assumptions for this plan:
- shortcut edits apply immediately after a valid capture; there is no separate Save button
- active bindings must include `Ctrl` or `Alt` as the base modifier, with optional `Shift`
- the `Keybinds` item appears only in the terminal surface context menu, not browser tabs or workspace menus

## Prerequisites
- Existing GTK4/libadwaita host build environment
- Existing host shortcut registry in `rust/limux-host-linux/src/shortcut_config.rs`
- Context7 GTK4 docs reviewed for `GtkPopover::set_autohide()` and `GtkEventControllerKey`

## Dependency Graph

```text
T1 ── T2 ──┐
           ├── T5 ── T6
T3 ── T4 ──┘
```

## Tasks

### T1: Strengthen the Canonical Shortcut Model for Editor-Backed Persistence
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`
- **description**: Extend the canonical shortcut module so it can serve a settings UI, not just startup loading. Add explicit helpers for enumerating every definition in display order, looking up the current/default display labels, validating user-captured bindings, and serializing only overrides back to config. Make the `Ctrl`/`Alt` base-modifier rule part of canonical validation so file-loaded bindings and UI-captured bindings follow the same contract. Keep duplicate-binding rejection centralized here. Because the file is the general `config.json`, add a read-modify-write path that preserves unrelated top-level settings and performs atomic writes (`create_dir_all`, temp file, rename) instead of overwriting the entire file blindly.
- **validation**: Pure unit tests cover: accepted combos (`Ctrl+H`, `Ctrl+Shift+H`, `Alt+X`), rejected combos (plain `H`, `Shift+H`, modifier-only keys), duplicate detection, serialization that writes only overrides/unbinds while preserving unrelated top-level config keys, and atomic write helpers that fail cleanly without corrupting an existing config file.
- **status**: Completed
- **log**: Added canonical host-binding validation in `shortcut_config.rs` so active shortcuts must include `Ctrl` or `Alt` and cannot target modifier-only keys. Added editor-facing helpers for default/current display labels plus override-only JSON serialization. Added atomic `config.json` merge-write support that preserves unrelated top-level settings and removes the `shortcuts` section when no overrides remain. Validation: `cargo test -p limux-host-linux shortcut_config::tests -- --nocapture`.
- **files edited/created**: `rust/limux-host-linux/src/shortcut_config.rs`, `terminal-keybinds-settings-plan.md`

### T2: Make Shortcut Updates Live at Runtime
- **depends_on**: [T1]
- **location**: `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`
- **description**: Refactor the current startup-only shortcut wiring into a live update path. `AppState` should own the effective shortcut registry in a mutable form, expose one helper that swaps in a newly validated registry, reapplies GTK accelerators through the `adw::Application`, and refreshes host-owned tooltip surfaces that show shortcuts today. Extend pane internals as needed so existing pane header buttons can refresh their tooltips instead of only reflecting shortcuts at creation time.
- **validation**: A single runtime update path exists for shortcut changes. After applying a new registry, the capture-phase handler, GTK accelerators, sidebar tooltip, and pane button tooltips all reflect the new bindings without reopening the app.
- **status**: Completed
- **log**: Moved GTK accelerator application into a reusable window-owned helper and added `apply_shortcut_config(...)` so a newly validated shortcut registry can replace the live `AppState` value, refresh GTK accelerators, update sidebar toggle tooltips, and refresh existing pane action tooltips through cached `PaneInternals` button refs. Validation: `cargo test -p limux-host-linux` and `cargo build -p limux-host-linux --features webkit`.
- **files edited/created**: `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/window.rs`, `terminal-keybinds-settings-plan.md`

### T3: Add a Terminal Context-Menu Entry Point for Keybind Settings
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/terminal.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Extend the existing terminal right-click menu to include a `Keybinds` item without regressing the current Copy/Paste/Split/Clear actions. Thread a first-class `on_open_keybinds` callback through `TerminalCallbacks` and `PaneCallbacks` so the terminal surface can ask the window layer to open the keybind editor using the same primary host codepath every time. Make the handoff explicit: selecting `Keybinds` should close the small context menu first and only then open the larger editor popover, ideally via an idle callback, so the new popover does not immediately dismiss or inherit the wrong transient parent state.
- **validation**: Right-clicking a terminal still shows the current context menu items plus `Keybinds`, activating `Keybinds` routes to one window-owned open-editor function rather than embedding editor state directly in `terminal.rs`, and the editor opens reliably after the context menu closes.
- **status**: Completed
- **log**: Added a `Keybinds` action to the terminal right-click menu and threaded a first-class `on_open_keybinds` callback through `TerminalCallbacks` and `PaneCallbacks` into `window.rs`. The terminal menu now pops down first and schedules the editor open via `glib::idle_add_local_once(...)` so the larger editor popover is opened from the window layer without nested-popover dismissal glitches. Validation: `cargo test -p limux-host-linux`.
- **files edited/created**: `rust/limux-host-linux/src/terminal.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/window.rs`, `terminal-keybinds-settings-plan.md`

### T4: Build the Keybinds Editor Popover Shell
- **depends_on**: [T1, T3]
- **location**: `rust/limux-host-linux/src/keybind_editor.rs` (new), with thin integration hooks in `rust/limux-host-linux/src/window.rs`
- **description**: Create the actual keybind editor popover as a dedicated module anchored from the terminal surface. The shell should be a `gtk::Popover` with `set_autohide(true)` so clicking outside dismisses it, plus a header row that includes a `Keybinds` title and an explicit close button at the top right. The body should be scrollable and render one row per canonical shortcut definition, showing the human-readable action label, the current binding, and the default binding as supporting text. Each binding cell should be an entry-like capture control, not a freeform text editor, so keyboard input is always normalized through the canonical shortcut path instead of mixing raw text editing with shortcut capture semantics.
- **validation**: Opening the editor from the terminal menu shows all current shortcut definitions, the top-right close button dismisses it, outside clicks also dismiss it with no orphaned popovers or double-parenting issues, and the binding cells present a clear idle/listening/error state without allowing arbitrary text entry.
- **status**: Completed
- **log**: Added the dedicated `keybind_editor.rs` module and its CSS, then built a window-owned `gtk::Popover` editor shell anchored to the terminal surface. The editor now renders every host shortcut with current binding plus default binding text, includes a top-right close button, and uses `set_autohide(true)` so outside clicks close it. Validation: `cargo test -p limux-host-linux` including `keybind_editor::tests::binding_button_label_prefers_current_binding_and_listening_state`.
- **files edited/created**: `rust/limux-host-linux/src/keybind_editor.rs`, `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/window.rs`, `terminal-keybinds-settings-plan.md`

### T5: Implement Capture, Conflict Handling, Persistence, and Live Apply
- **depends_on**: [T2, T4]
- **location**: `rust/limux-host-linux/src/keybind_editor.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/shortcut_config.rs`
- **description**: Implement the row-level editing workflow. Clicking a shortcut field should enter a clear listening state, attach a `GtkEventControllerKey`, and capture the next non-modifier key combo. Valid captures should be normalized through the canonical shortcut model, rejected if they do not use `Ctrl` or `Alt`, rejected if they conflict with another active binding, merged atomically into the `shortcuts` section of `config.json`, reloaded through the canonical config loader, and only then applied live through the shared runtime update helper. Invalid captures or write failures should leave the previous binding intact and surface a row-local error message instead of silently failing.
- **validation**: A remap such as `Ctrl+H` for `Split Right` can be captured from the UI, written to config, reloaded, applied live, and used immediately. Conflicts, invalid combos, and disk-write failures show deterministic errors, and the previous working binding remains active until a valid replacement is both persisted and reloaded successfully.
- **status**: Completed
- **log**: The editor’s binding controls now enter a listening state, capture the next key combo through `GtkEventControllerKey`, validate it through the canonical shortcut model, persist it through `write_shortcuts(...)`, reload the effective config, and apply it live with `apply_shortcut_config(...)`. Invalid combos and duplicate bindings surface row-local errors without replacing the prior working binding. Validation: `cargo test -p limux-host-linux` and `cargo build -p limux-host-linux --features webkit`.
- **files edited/created**: `rust/limux-host-linux/src/keybind_editor.rs`, `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`, `terminal-keybinds-settings-plan.md`

### T6: Add Regression Coverage and Manual Verification Notes
- **depends_on**: [T5]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/keybind_editor.rs`, `rust/limux-host-linux/src/window.rs`, `docs/shortcut-remap-testing.md` or a new focused settings test doc if cleaner
- **description**: Add focused regression coverage for the editor contract and update the manual verification doc to include the new settings surface. Favor pure tests around validation, serialization, and row-state helpers where possible, and add narrow UI-helper tests for row text or error formatting when GTK startup is avoidable. Document a manual smoke test that exercises: open terminal menu, open `Keybinds`, capture a valid combo, reject an invalid combo, reject a duplicate combo, close via X, and close via outside click.
- **validation**: `cargo test -p limux-host-linux` covers the new canonical validation and persistence behavior, and the manual test doc provides a deterministic checklist for the interactive GTK-only behaviors.
- **status**: Completed
- **log**: Added focused keybind-editor helper coverage for binding-label and validation-error messaging, then updated `docs/shortcut-remap-testing.md` with the new terminal `Keybinds` popover flow, immediate live-apply behavior, close affordances, remap checklist, and validation checklist. Validation: `cargo test -p limux-host-linux`, `cargo build -p limux-host-linux --features webkit`, and live host launch via `cargo run -p limux-host-linux --features webkit --bin limux`.
- **files edited/created**: `rust/limux-host-linux/src/keybind_editor.rs`, `docs/shortcut-remap-testing.md`, `terminal-keybinds-settings-plan.md`

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1, T3 | Immediately |
| 2 | T2, T4 | T1 and T3 complete as required |
| 3 | T5 | T2 and T4 complete |
| 4 | T6 | T5 complete |

## Testing Strategy
- Add pure unit tests in `shortcut_config.rs` for editor-facing validation and override serialization.
- Add pure unit tests in `shortcut_config.rs` for config merge behavior so editing `shortcuts` does not delete unrelated future settings in `config.json`.
- Add focused helper tests for any non-trivial row-state formatting or conflict messaging extracted out of GTK widget callbacks.
- Run `cargo test -p limux-host-linux`.
- Run `cargo build -p limux-host-linux --features webkit`.
- Manually validate the GTK flow with:

```bash
LD_LIBRARY_PATH="/home/willr/Applications/cmux-linux/cmux/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
cargo run -p limux-host-linux --features webkit --bin limux
```

- Manual acceptance checklist:
  - right-click inside a terminal shows `Keybinds`
  - selecting `Keybinds` opens the editor popover
  - all shortcut rows show current binding and default binding
  - clicking the top-right X closes the editor
  - clicking outside the editor closes it
  - clicking a binding field enters listening mode
  - `Ctrl+H` can be assigned to `Split Right`
  - invalid combos without `Ctrl` or `Alt` are rejected
  - duplicate active combos are rejected
  - accepted remaps persist to `~/.config/limux/config.json`
  - accepted remaps take effect immediately in the running app
  - reopening the editor and relaunching Limux both show the persisted remap

## Risks & Mitigations
- The current shortcut registry is loaded once and cloned into panes at creation time.
  - Mitigation: make live shortcut application a first-class `window.rs` responsibility and refresh existing tooltip surfaces from stored widget refs instead of reconstructing panes.
- `config.json` is a shared preferences file, so writing only shortcut data can accidentally erase unrelated settings.
  - Mitigation: implement shortcut persistence as a merge into the existing top-level JSON object plus atomic temp-file rename.
- Nested or chained popovers can leak or double-parent if each surface owns its own editor instance.
  - Mitigation: keep one window-owned open-editor entrypoint, ensure every popover unparents itself on `closed`, and open the editor only after the terminal context menu has fully popped down.
- GTK accelerator updates can drift from capture-phase updates if they are applied through separate codepaths.
  - Mitigation: use one shared apply helper that updates both the `AppState` shortcut registry and the GTK action accelerators in the same step.
- Key capture can accidentally swallow input meant for Ghostty if listening state leaks outside the editor.
  - Mitigation: keep capture scoped to the editor field with `GtkEventControllerKey`, and only enter capture mode after the user explicitly clicks a binding field.
- The current config system accepts more modifier combinations than the new UI contract allows.
  - Mitigation: move the `Ctrl`/`Alt` requirement into canonical shortcut validation so the file format and UI stay consistent.
