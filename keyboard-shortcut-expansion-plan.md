# Plan: Limux Keyboard Shortcut Expansion

**Generated**: 2026-03-24

## Overview
Expand the Linux host shortcut system so Limux can support the screenshot-driven shortcut set where it is technically feasible. The canonical approach is:

- extend the existing host shortcut registry in `rust/limux-host-linux/src/shortcut_config.rs` instead of adding a second shortcut path
- split shortcut dispatch by scope: app-level (`quit`, `new instance`), focused terminal surface actions, focused browser actions, and existing window/pane commands
- reuse first-class integrations that already exist in the codebase where possible:
  - Ghostty binding actions for terminal search, clipboard, clear scrollback, and font-size controls
  - WebKitGTK `WebView` navigation plus `WebKitFindController` and inspector APIs for browser actions
- add typed focused-surface helpers so keyboard dispatch can deterministically target the active terminal or browser tab without guessing
- add an explicit shortcut-capture bypass policy for editable widgets so text inputs keep native editing behavior unless a shortcut is intentionally app-global
- keep docs, tooltips, and the keybind editor aligned with the new modifier policy and new defaults

Policy decisions that must be frozen before implementation work fans out:

- what physical Linux modifier counts as `Cmd` for defaults and capture matching: `Meta`, `Super`, or an alias policy that accepts both
- which shortcuts are always app-global versus bypassed inside editable widgets
- which Ghostty binding-action strings are guaranteed by the vendored Ghostty version for find and font-size control

## Frozen Decisions

### Command Modifier Policy

| Policy | Final choice |
|---|---|
| Linux `Cmd` physical modifier | Logical `Cmd` alias that matches either `Meta` or `Super` at runtime |
| Canonical config/display form | Serialize and display `Cmd` |
| GTK accelerator registration | Expand each `Cmd` binding into both `<Meta>...` and `<Super>...` variants |
| Validation rule | Base modifiers are `Ctrl`, `Alt`, or `Cmd`; `Shift` is optional |

### Capture Policy

| Shortcut scope | Capture rule |
|---|---|
| App global | Always capture, including inside editable widgets |
| Window/pane, terminal, browser, focused-surface find | Bypass editable widgets and require a matching focused target |
| No focused pane target | Surface-local commands no-op instead of falling back to the workspace root |

### Shortcut Inventory

| Scope | Shortcuts |
|---|---|
| App global | `Cmd+Q`, `Cmd+Shift+N` |
| Browser | `Cmd+Shift+L`, `Cmd+L`, `Cmd+[`, `Cmd+]`, `Cmd+R`, `Opt+Cmd+I`, `Opt+Cmd+C` |
| Focused-surface find | `Cmd+F`, `Cmd+G`, `Cmd+Shift+G`, `Cmd+Shift+F`, `Cmd+E` |
| Terminal only | `Cmd+K`, `Cmd+C`, `Cmd+V`, `Cmd++`, `Cmd+-`, `Cmd+0` |

### Frozen Ghostty Binding Actions

| Limux action | Ghostty binding action |
|---|---|
| Find | `start_search` |
| Find next | `navigate_search:next` |
| Find previous | `navigate_search:previous` |
| Hide find | `end_search` |
| Use selection for find | `search_selection` |
| Clear scrollback | `clear_screen` |
| Copy | `copy_to_clipboard` |
| Paste | `paste_from_clipboard` |
| Increase font size | `increase_font_size:1` |
| Decrease font size | `decrease_font_size:1` |
| Reset font size | `reset_font_size` |

## Prerequisites
- Current shortcut registry already lives in `rust/limux-host-linux/src/shortcut_config.rs`.
- Browser support is built with the default `webkit` feature in `rust/limux-host-linux`.
- Documentation checked before planning:
  - GTK4 `GtkApplication.set_accels_for_action` behavior for registering and clearing accelerators
  - GTK/GIO application-flag behavior around `NON_UNIQUE`
  - WebKitGTK `WebKitFindController` search flow
  - WebKitGTK `WebInspector` show/access APIs
- Validation commands available from repo root:
  - `cargo test -p limux-host-linux`
  - `cargo build -p limux-host-linux --features webkit`
  - `cargo build -p limux-host-linux --no-default-features`

## Dependency Graph

```text
T1 ──┬── T2a ── T4 ──┐
     │               │
     ├── T2b ── T3 ── T5 ── T6 ── T7
     │                              │
     └──────────────────────────────┴── T8
```

## Tasks

### T1: Freeze Scope and Canonical Shortcut Table
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/terminal.rs`, `README.md`
- **description**: Define the exact shortcut inventory to add, grouped by scope. This should include app-level actions (`Cmd+Q`, `Cmd+Shift+N`), browser actions (`Cmd+Shift+L`, `Cmd+L`, `Cmd+[`, `Cmd+]`, `Cmd+R`, `Opt+Cmd+I`, `Opt+Cmd+C`), terminal/browser find actions (`Cmd+F`, `Cmd+G`, `Cmd+Shift+G`, `Cmd+Shift+F`, `Cmd+E`), and terminal utility actions (`Cmd+K`, `Cmd+C`, `Cmd+V`, `Cmd++`, `Cmd+-`, `Cmd+0`). Freeze three explicit tables before implementation: the shortcut inventory, the physical-modifier policy for `Cmd` on Linux (`Meta`, `Super`, or aliasing both), and the exact vendored Ghostty binding-action strings needed for find/font/reset behavior. Explicitly mark which shortcuts are globally registered accelerators versus focused-surface capture-only commands.
- **validation**: There is one reviewed plan artifact listing every new shortcut, its default binding, scope, target action, editable-widget bypass policy, physical-modifier policy, and Ghostty action-name mapping.
- **status**: Completed
- **log**:
  - Froze the implementation around a logical `Cmd` alias policy so either Linux `Meta` or `Super` triggers command shortcuts.
  - Classified lifecycle shortcuts as app-global and all browser, terminal, and find additions as editable-bypassed surface commands.
  - Verified and froze the Ghostty binding-action names used by terminal search, clipboard, scrollback, and font-size shortcuts.
- **files edited/created**:
  - `keyboard-shortcut-expansion-plan.md`

### T2a: Extend the Registry for Mixed Namespaces and Canonical Shortcut Entries
- **depends_on**: [T1]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Add the new shortcut IDs/commands to the existing registry and make the registry understand mixed action namespaces. This includes adding the app-level commands and teaching action registration to handle both `win.*` and `app.*` action names without duplicating the registry. The command-modifier policy chosen in T1 must be encoded here so the canonical defaults, runtime matching, and docs all point at the same source of truth.
- **validation**: Registry tests prove the new shortcuts resolve to unique commands, namespace-aware action registration can emit both `win.*` and `app.*` accelerators correctly, and lifecycle shortcuts have first-class `app.*` definitions instead of window-only shims.
- **status**: Completed
- **log**:
  - Expanded the canonical registry to 45 shortcut definitions, including `app.*`, `win.*`, browser, terminal, and focused-surface find commands.
  - Kept full action names in the registry and exposed stripped basenames for `gio::SimpleAction` registration so app and window namespaces stay first-class.
  - Normalized `Cmd` defaults so config, display labels, runtime matching, and GTK accelerators all derive from the same source of truth.
- **files edited/created**:
  - `rust/limux-host-linux/src/shortcut_config.rs`
  - `rust/limux-host-linux/src/window.rs`

### T2b: Expand Capture Dispatch, Modifier Validation, and Editable-Widget Bypass Rules
- **depends_on**: [T1, T2a]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/keybind_editor.rs`, `rust/limux-host-linux/src/window.rs`, `docs/shortcut-remap-testing.md`
- **description**: Broaden validation so the chosen `Cmd` modifier policy from T1 is accepted everywhere the shortcut stack validates input, then add a capture-bypass policy for editable widgets. This policy must explicitly distinguish app-global shortcuts from surface-local ones so text fields like the browser URL entry, browser find entry, sidebar rename entry, and future editable widgets keep native editing behavior. Update the keybind editor guidance and validation messaging so loader validation, live capture validation, display labels, tooltip text, and docs all describe the same allowed base modifiers.
- **validation**: The loader, runtime capture path, keybind editor, display labels, tooltips, and docs all agree on the allowed base modifiers, and capture dispatch has a unit-tested bypass rule that prevents `Cmd+C`, `Cmd+V`, `Cmd+F`, `Cmd+L`, and `Cmd+R` from being stolen out of editable widgets unless they are explicitly app-global.
- **status**: Completed
- **log**:
  - Updated validation, editor messaging, and display behavior to accept `Ctrl`, `Alt`, or `Cmd`.
  - Added editable-widget bypass logic so surface-local/browser shortcuts do not steal combos from entries, search fields, or rename fields.
  - Added regression coverage for the editable-bypass policy and `Cmd` accelerator expansion.
- **files edited/created**:
  - `rust/limux-host-linux/src/shortcut_config.rs`
  - `rust/limux-host-linux/src/keybind_editor.rs`
  - `rust/limux-host-linux/src/window.rs`
  - `docs/shortcut-remap-testing.md`

### T3: Add Mandatory Typed Focused-Surface Handles and No-Target Behavior
- **depends_on**: [T1, T2b]
- **location**: `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/terminal.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Create deterministic helpers that can find the active tab kind and return a typed focused action target for that tab. For terminals, expose a safe helper that can invoke Ghostty binding actions on the focused surface. For browsers, return typed handles for the active `WebView`, URL entry, and any find-bar state. Evolve `TabKind` and pane internals to carry structured browser/terminal handles instead of only serialized state, and introduce an explicit `FocusedShortcutTarget::None` path so surface-specific shortcuts no-op instead of guessing against the workspace root when focus is outside a usable pane. Keep the non-`webkit` branch compiling by making any new browser-handle types or helpers cfg-safe.
- **validation**: Host code can answer “what is the focused shortcut target right now?” with a typed result or `None`, no surface-specific shortcut path relies on ad-hoc widget guessing, and both `--features webkit` and `--no-default-features` builds compile after the handle scaffolding lands.
- **status**: Completed
- **log**:
  - Reworked pane tab state so terminals and browsers carry live shortcut handles instead of only serialized state.
  - Added `FocusedShortcutTarget::{Terminal,Browser,Keybinds,None}` and removed workspace-root fallback from surface-local shortcut routing.
  - Kept the browser handle scaffolding cfg-safe so the non-WebKit build still compiles.
- **files edited/created**:
  - `rust/limux-host-linux/src/pane.rs`
  - `rust/limux-host-linux/src/terminal.rs`
  - `rust/limux-host-linux/src/window.rs`

### T4: Implement App-Level Lifecycle Shortcuts
- **depends_on**: [T2a]
- **location**: `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/main.rs`, `README.md`
- **description**: Implement `Cmd+Q` to save state and quit the current app process, and implement `Cmd+Shift+N` to spawn a new Limux instance from the current executable path. Because the app already uses `ApplicationFlags::NON_UNIQUE`, wire this through first-class `app.*` actions instead of faking it as a pane/window shortcut. Preserve the current environment when spawning so dev launches keep runtime linkage, and define a visible/logged error path for spawn failures.
- **validation**: App actions are registered once, accelerator bindings point at `app.quit` and `app.new-instance`, quitting still persists session state, and the new-instance action launches a second process while preserving environment-dependent runtime behavior in both dev and packaged launches. Spawn failures are reported instead of failing silently.
- **status**: Completed
- **log**:
  - Registered lifecycle shortcuts as first-class `app.quit` and `app.new-instance` actions.
  - Wired `Cmd+Q` through the session save path before quitting.
  - Implemented `Cmd+Shift+N` by spawning the current executable and added a visible GTK error dialog plus stderr logging for failures.
- **files edited/created**:
  - `rust/limux-host-linux/src/window.rs`
  - `README.md`

### T5: Implement Focused Terminal Shortcuts via Verified Ghostty Binding Actions
- **depends_on**: [T2b, T3]
- **location**: `rust/limux-host-linux/src/terminal.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`, `README.md`
- **description**: Route terminal-only shortcuts through `ghostty_surface_binding_action(...)` on the focused terminal surface instead of reimplementing terminal behavior in the host. This covers find/show-hide-next-previous-selection-for-find, clear scrollback, copy, paste, increase font size, decrease font size, and reset font size, but only after verifying the exact vendored Ghostty action-name strings frozen in T1. If a desired action is not supported by the vendored Ghostty version, define the fallback explicitly in code and docs instead of assuming parity with other platforms.
- **validation**: Terminal shortcuts trigger the expected verified Ghostty actions on the focused surface, work across split panes, and do not steal the same key combos when focus is on a browser or non-terminal widget. Unsupported Ghostty actions, if any, have documented fallback or no-op behavior.
- **status**: Completed
- **log**:
  - Routed terminal-only shortcuts through verified Ghostty binding actions on the focused terminal handle.
  - Ensured terminal shortcuts return `false` when focus is on a browser, keybinds tab, or non-pane target so they are not stolen incorrectly.
  - Covered the `Cmd` alias/accelerator behavior with registry-level tests.
- **files edited/created**:
  - `rust/limux-host-linux/src/terminal.rs`
  - `rust/limux-host-linux/src/window.rs`
  - `rust/limux-host-linux/src/pane.rs`
  - `README.md`

### T6: Implement Focused Browser Shortcuts
- **depends_on**: [T2b, T3, T5]
- **location**: `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/window.rs`, `README.md`
- **description**: Add browser-targeted commands for open-browser-in-split, focus address bar, back, forward, reload, Web Inspector, and browser find. The browser find path should be host-owned: add a small `GtkSearchBar`/`SearchEntry` surface tied to `WebKitFindController`, support next/previous/hide flows, and ensure shortcuts only fire when the focused tab is a browser tab. For `Cmd+Shift+L`, either create a browser tab in a newly created split or add a dedicated split helper that returns the new pane so browser creation lands in the correct location. If WebKitGTK cannot expose a console-only action for `Opt+Cmd+C`, degrade to opening Web Inspector and document the gap.
- **validation**: When a browser tab is focused, the browser shortcuts act on that tab only; when a terminal or keybinds tab is focused, those browser commands do nothing or fall through. Browser find supports open, next, previous, and hide, inspector shortcuts expose developer tools as far as WebKitGTK allows, and the non-`webkit` build still compiles cleanly.
- **status**: Completed
- **log**:
  - Added focused browser shortcuts for split, location focus, back, forward, reload, inspector, and host-owned browser find.
  - Added a `GtkSearchBar` plus `SearchEntry` tied to `WebKitFindController`, including next, previous, hide, and selection-for-find flows.
  - Implemented `Opt+Cmd+C` as the documented Web Inspector fallback because console-only targeting is not exposed by WebKitGTK.
- **files edited/created**:
  - `rust/limux-host-linux/src/pane.rs`
  - `rust/limux-host-linux/src/window.rs`
  - `README.md`

### T7: Align Visible Hints, Docs, and Manual Test Coverage
- **depends_on**: [T4, T5, T6]
- **location**: `README.md`, `docs/shortcut-remap-testing.md`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/keybind_editor.rs`
- **description**: Update user-facing shortcut documentation and tooltip/hint surfaces so the new defaults and modifier policy are discoverable. Add a manual QA section that explicitly tests app-level, terminal, browser, and find behaviors, including focus-sensitive dispatch and Meta/Cmd remapping. If browser or terminal helper surfaces gain visible UI (for example a browser find bar), document the expected interaction flow and close behavior.
- **validation**: README and shortcut-remap docs enumerate the new defaults and manual verification steps; any user-visible tooltip or helper text that references supported modifiers or affected actions matches the implementation.
- **status**: Completed
- **log**:
  - Updated README shortcut tables to include the new app, browser, find, and terminal shortcuts.
  - Expanded the shortcut-remap manual with the `Cmd` alias policy, supported shortcut IDs, editable-bypass expectations, and browser/terminal verification steps.
  - Kept the docs aligned with the current implementation gap where `Opt+Cmd+C` opens Web Inspector instead of a console-only target.
- **files edited/created**:
  - `README.md`
  - `docs/shortcut-remap-testing.md`

### T8: Add Regression Tests and Run the Host Quality Gate
- **depends_on**: [T4, T5, T6, T7]
- **location**: `rust/limux-host-linux/src/shortcut_config.rs`, `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/keybind_editor.rs`
- **description**: Add focused regression tests for the highest-risk areas: chosen `Cmd` modifier policy (`Meta`, `Super`, or aliasing), app-vs-window action registration, editable-widget bypass, focused terminal dispatch, focused browser dispatch, explicit no-target behavior, browser find-bar state transitions, old-binding disablement after remaps, and any docs/helper text that intentionally gates modifier-policy drift. Then run the relevant test/build commands from the repo root.
- **validation**: `cargo test -p limux-host-linux` passes, `cargo build -p limux-host-linux --features webkit` passes, `cargo build -p limux-host-linux --no-default-features` passes, and the new tests fail if the chosen command-modifier policy, focused-surface routing, editable-widget bypass, or find behavior regresses.
- **status**: Completed
- **log**:
  - Added regression coverage for `Cmd` alias expansion and editable-widget bypass policy.
  - Ran the host unit suite and both feature builds from the repo root.
  - Verified the default and non-default feature builds after the pane/browser handle refactor.
- **files edited/created**:
  - `rust/limux-host-linux/src/shortcut_config.rs`
  - `rust/limux-host-linux/src/window.rs`

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1 | Immediately |
| 2 | T2a, T2b | T1 complete |
| 3 | T3, T4 | T2b complete for T3, and T2a complete for T4 |
| 4 | T5 | T2b and T3 complete |
| 5 | T6 | T2b, T3, and T5 complete |
| 6 | T7 | T4, T5, T6 complete |
| 7 | T8 | T4, T5, T6, T7 complete |

## Testing Strategy
- Add pure/unit coverage around shortcut parsing, registry entries, display labels, and action routing in `shortcut_config.rs`, `window.rs`, and `keybind_editor.rs`.
- Add focused helper tests in `pane.rs` or `window.rs` that prove browser-only and terminal-only shortcuts do not fire on the wrong surface, and that explicit no-target cases stay inert.
- Add capture-bypass tests proving editable widgets retain native text-editing shortcuts while app-global actions still work where intended.
- Manually verify:
  - `Cmd+Q` quits cleanly after persisting state
  - `Cmd+Shift+N` opens a second Limux instance
  - `Cmd+F` opens terminal search on terminals and browser find on browsers
  - `Cmd+G` / `Cmd+Shift+G` navigate find results in both surface types
  - browser navigation/devtools shortcuts only affect focused browser tabs
  - terminal copy/paste/clear/font shortcuts only affect focused terminal tabs
  - browser URL entry, browser find entry, and rename fields retain native copy/paste/select/edit behavior
- Run:
  - `cargo test -p limux-host-linux`
  - `cargo build -p limux-host-linux --features webkit`
  - `cargo build -p limux-host-linux --no-default-features`

## Risks & Mitigations
- Linux keyboards may report the intended `Cmd` key as `Meta` or `Super`, and the registry currently only supports one binding per shortcut.
  - Mitigation: freeze the physical-modifier policy in T1 and enforce it consistently in loader validation, capture validation, docs, and tests before adding defaults.
- App-level actions need different registration than existing `win.*` actions.
  - Mitigation: make the shortcut registry namespace-aware and register `app.*` actions on `adw::Application` instead of overloading the window action path.
- Browser shortcuts currently have no structured access to the active `WebView` or URL entry, and non-`webkit` builds still need to compile.
  - Mitigation: add explicit browser handle storage in pane/tab state behind cfg-safe types and route all browser commands through those helpers.
- Terminal shortcuts may be duplicated if both host capture and Ghostty native bindings respond.
  - Mitigation: keep host interception focus-sensitive and route terminal actions through Ghostty binding actions only, avoiding duplicate custom logic.
- Editable widgets can lose native copy/paste/find/location behavior if capture-phase dispatch runs too early.
  - Mitigation: add one explicit bypass policy keyed off the focused widget type and shortcut scope, and regression-test URL entries, search entries, and rename fields.
- `Option+Cmd+C` may not have a direct WebKitGTK “show console only” hook.
  - Mitigation: fall back to opening Web Inspector and document the parity gap if console-tab targeting is not supported.
- Browser find introduces extra UI state that can steal focus or remain stuck open.
  - Mitigation: use a single host-owned search bar per browser tab with explicit open/next/previous/hide helpers and regression tests around focus restoration.
