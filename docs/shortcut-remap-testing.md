# Limux Shortcut Remapping

This document explains how the Linux host shortcut system works and how to test it manually.

## What It Does

Limux has a host-owned shortcut registry in `rust/limux-host-linux/src/shortcut_config.rs`.

That registry is the single source of truth for:

- default shortcut bindings
- user overrides from config
- GTK app accelerators
- capture-phase host shortcut dispatch
- visible tooltip text for shortcut-backed UI actions

Ghostty config is not involved. Ghostty still owns terminal behavior once Limux decides not to intercept a key.

## Config File Location

Limux reads shortcuts from:

```text
~/.config/limux/shortcuts.json
```

That path comes from `dirs::config_dir()/limux/shortcuts.json`.

If the file is missing, Limux uses built-in defaults.

Older Limux builds stored shortcut overrides under the top-level `shortcuts`
key in `~/.config/limux/config.json`. On startup, if `shortcuts.json` does not
exist, Limux migrates that legacy `shortcuts` object into `shortcuts.json` and
leaves `config.json` untouched.

## Important Runtime Behavior

- Shortcuts are loaded at startup.
- When you change one through the `Keybinds` editor, Limux saves that entry, keeps the other entries in the file, and applies the new binding immediately in the running app.
- If you edit `~/.config/limux/shortcuts.json` by hand outside the app, restart Limux to pick up those changes.
- If the config file is invalid or unreadable, Limux falls back to defaults and prints a warning to stderr.
- If two overrides resolve to the same binding, Limux rejects the override set and falls back to defaults.
- If an override uses the default binding of another shortcut, the override wins. The other shortcut has no binding, and Limux prints a warning.
- Unknown shortcut IDs, and entries that Limux cannot read, are ignored with a warning. The other entries still apply.
- `null` or `""` unbinds a shortcut.
- Host shortcuts must use `Ctrl`, `Alt`, or `Cmd` as the base modifier unless the shortcut explicitly allows a bare function key, such as the default `F11` fullscreen binding. `Shift` can be added on top of a modified shortcut.
- Most default shortcuts use `Ctrl`; fullscreen defaults to `F11`.
- `Cmd` is a logical Limux modifier that matches either Linux `Meta` or Linux `Super` for custom remaps. `Opt` is another name for `Alt`.
- App-global shortcuts still fire inside editable widgets, but surface and browser shortcuts bypass editable widgets so native text editing keeps working.

## Keybinds Editor

The terminal right-click menu includes `Keybinds`, which opens the editor in a tab of that pane. `Settings` shows the same editor on its `Keybindings` page.

The editor:

- lists every host-owned shortcut
- shows the current binding
- shows the default binding
- lets you click a binding pill to enter listening mode
- takes the keys you press while it listens, so a shortcut that you press does not run

Capture rules:

- valid examples:
  - `Ctrl+H`
  - `Ctrl+Shift+H`
  - `Alt+X`
  - `Ctrl+L`
- rejected examples:
  - plain `H`
  - `Shift+H`
  - modifier-only keys like `Ctrl`

If a capture is invalid or duplicates another active shortcut, the row shows an inline error and keeps the previous working binding.

## Config Format

Top-level shape:

```json
{
  "shortcuts": {
    "toggle_sidebar": "<Ctrl><Alt>b",
    "split_right": null,
    "new_terminal": ""
  }
}
```

Rules:

- Keys must be under `"shortcuts"`.
- Values must be either:
  - a GTK-style accelerator string like `"<Ctrl><Alt><Shift>n"`
  - `null` to unbind
  - `""` to unbind
- Omitted keys keep their defaults.

## Supported Shortcut IDs

These are the current supported config keys and defaults:

| Config key | Default |
|---|---|
| `new_workspace` | `<Ctrl><Alt><Shift>n` |
| `close_workspace` | `<Ctrl><Alt><Shift>w` |
| `quit_app` | `<Ctrl><Alt>q` |
| `new_instance` | `<Ctrl><Alt>n` |
| `toggle_sidebar` | `<Ctrl><Alt>m` |
| `toggle_top_bar` | `<Ctrl><Alt><Shift>m` |
| `toggle_fullscreen` | `F11` |
| `next_workspace` | `<Ctrl><Alt>Page_Down` |
| `prev_workspace` | `<Ctrl><Alt>Page_Up` |
| `cycle_tab_prev` | `<Ctrl><Alt><Shift>Left` |
| `cycle_tab_next` | `<Ctrl><Alt><Shift>Right` |
| `split_down` | `<Ctrl><Alt><Shift>d` |
| `new_terminal_in_focused_pane` | `<Ctrl><Shift>t` |
| `split_right` | `<Ctrl><Alt>d` |
| `close_focused_pane` | `<Ctrl><Alt>w` |
| `close_focused_tab` | `<Ctrl><Shift>w` |
| `toggle_focused_pane_zoom` | `<Ctrl><Shift>z` |
| `new_terminal` | `<Ctrl><Alt>t` |
| `focus_left` | `<Ctrl><Alt>Left` |
| `focus_right` | `<Ctrl><Alt>Right` |
| `focus_up` | `<Ctrl><Alt>Up` |
| `focus_down` | `<Ctrl><Alt>Down` |
| `activate_workspace_1` | `<Ctrl><Alt>1` |
| `activate_workspace_2` | `<Ctrl><Alt>2` |
| `activate_workspace_3` | `<Ctrl><Alt>3` |
| `activate_workspace_4` | `<Ctrl><Alt>4` |
| `activate_workspace_5` | `<Ctrl><Alt>5` |
| `activate_workspace_6` | `<Ctrl><Alt>6` |
| `activate_workspace_7` | `<Ctrl><Alt>7` |
| `activate_workspace_8` | `<Ctrl><Alt>8` |
| `activate_last_workspace` | `<Ctrl><Alt>9` |
| `recent_workspace` | `<Ctrl><Alt>o` |
| `next_unread_terminal` | `<Ctrl><Alt>u` |
| `prev_unread_terminal` | `<Ctrl><Alt><Shift>u` |
| `open_browser_in_split` | `<Ctrl><Shift>l` |
| `browser_focus_location` | `<Ctrl>l` |
| `browser_back` | `<Ctrl>bracketleft` |
| `browser_forward` | `<Ctrl>bracketright` |
| `browser_reload` | `<Ctrl>r` |
| `browser_inspector` | `<Ctrl><Alt>i` |
| `browser_console` | `<Ctrl><Alt>c` |
| `surface_find` | `<Ctrl><Alt>f` |
| `surface_find_next` | `<Ctrl><Alt>g` |
| `surface_find_previous` | `<Ctrl><Alt><Shift>g` |
| `surface_find_hide` | `<Ctrl><Alt><Shift>f` |
| `surface_use_selection_for_find` | `<Ctrl><Alt>e` |
| `terminal_clear_scrollback` | `<Ctrl><Alt>k` |
| `terminal_copy` | `<Ctrl><Shift>c` |
| `terminal_paste` | `<Ctrl><Shift>v` |
| `terminal_increase_font_size` | `<Ctrl><Alt>equal` |
| `terminal_decrease_font_size` | `<Ctrl><Alt>minus` |
| `terminal_reset_font_size` | `<Ctrl><Alt><Shift>0` |

## Dispatch Model

There are two host shortcut paths, both driven by the same resolved registry:

1. GTK accelerators
   - Used for:
     - `new_workspace`
     - `close_workspace`
     - `quit_app`
     - `new_instance`
     - `toggle_sidebar`
     - `toggle_top_bar`
     - `toggle_fullscreen`
     - `next_workspace`
     - `prev_workspace`
2. Capture-phase key dispatch
   - Used for everything in the table above, including the GTK-backed actions
   - Surface commands resolve the focused pane target first:
     - terminal target for Ghostty binding actions
     - browser target for WebKit navigation, find, and inspector actions
     - `None` when focus is outside a usable pane

That means a remap changes both the GTK accelerator registration and the capture-phase match.

## Pass-Through Behavior

If a key combo does not match a resolved Limux shortcut, Limux does not intercept it and Ghostty receives it.

That means terminal-native combos like these should pass through unless you explicitly bind them in Limux:

- `Ctrl+C`
- `Ctrl+L`
- `Ctrl+R`
- plain typing
- `Enter`

Editable browser fields should also retain native behavior for:

- `Ctrl+C`
- `Ctrl+V`
- `Ctrl+F`
- `Ctrl+L`
- `Ctrl+R`

This is the behavior you want when testing that unbound shortcuts stop being stolen by the host.

## Visible Tooltip Behavior

These UI surfaces currently reflect shortcut overrides:

- sidebar collapse button
- sidebar expand button
- pane header buttons for:
  - new terminal tab
  - split right
  - split down
  - close pane

These surfaces do not currently show a shortcut suffix:

- new browser tab button
- browser navigation buttons (`Back`, `Forward`, `Reload`)
- browser find bar controls

Note:

- `new_terminal` and `new_terminal_in_focused_pane` both dispatch to the same terminal-tab creation command today.
- The pane header tooltip uses `new_terminal`, not `new_terminal_in_focused_pane`.

## Launch Commands

From the repo root:

```bash
cargo test -p limux-host-linux
cargo build -p limux-host-linux --features webkit
cargo build -p limux-host-linux --no-default-features
```

Run the app for manual testing:

```bash
LD_LIBRARY_PATH="/home/willr/Applications/cmux-linux/cmux/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
cargo run -p limux-host-linux --features webkit --bin limux
```

## Manual Test Plan

### 1. Baseline Defaults

Remove or move the config file out of the way:

```bash
trash ~/.config/limux/shortcuts.json
```

Launch Limux and verify:

- `Ctrl+Alt+M` toggles the sidebar
- `Ctrl+Alt+Shift+M` toggles the top bar
- `F11` toggles fullscreen
- `Ctrl+Alt+T` opens a terminal tab
- `Ctrl+Alt+D` splits right
- `Ctrl+Alt+Shift+D` splits down
- `Ctrl+W` reaches the focused terminal
- `Ctrl+Shift+W` closes the focused tab
- `Ctrl+Alt+W` closes the focused pane
- `Ctrl+Alt+Page_Down` and `Ctrl+Alt+Page_Up` switch workspaces
- pane button tooltips show the default shortcut suffixes where applicable
- `Ctrl+Alt+Q` quits Limux
- `Ctrl+Alt+N` opens a second Limux instance
- `Ctrl+Alt+K` clears terminal scrollback
- `Ctrl+Alt+Shift+0` resets terminal font size

### 2. Remap One Shortcut

Create:

```json
{
  "shortcuts": {
    "toggle_sidebar": "<Ctrl><Alt>b"
  }
}
```

Restart Limux and verify:

- `Ctrl+Alt+B` toggles the sidebar
- `Ctrl+Alt+M` no longer toggles the sidebar

### 3. Unbind One Shortcut

Create:

```json
{
  "shortcuts": {
    "split_right": null
  }
}
```

Restart Limux and verify:

- `Ctrl+Alt+D` no longer triggers split-right in Limux
- the split-right button tooltip no longer shows a shortcut suffix
- in a terminal pane, `Ctrl+D` reaches the terminal app because it is not a default Limux shortcut

### 4. Verify Pane Tooltip Remap

Create:

```json
{
  "shortcuts": {
    "new_terminal": "<Ctrl><Alt>y",
    "close_focused_pane": "<Ctrl><Alt>BackSpace"
  }
}
```

Restart Limux and verify:

- pane button tooltips show `Ctrl+Alt+Y` and `Ctrl+Alt+Backspace`
- `Ctrl+Alt+Y` opens a terminal tab
- `Ctrl+Alt+T` no longer opens a terminal tab
- `Ctrl+Alt+BackSpace` closes the focused pane
- `Ctrl+Alt+W` no longer closes the pane

### 5. Duplicate-Binding Rejection

Create:

```json
{
  "shortcuts": {
    "toggle_sidebar": "<Ctrl><Alt>b",
    "split_right": "<Ctrl><Alt>b"
  }
}
```

Restart Limux from a terminal and verify:

- Limux prints a warning about duplicate bindings
- Limux falls back to defaults
- `Ctrl+Alt+M` toggles the sidebar
- `Ctrl+Alt+D` still splits right

### 6. Open The Keybinds Editor

Launch Limux, right-click inside a terminal, and verify:

- the terminal context menu contains `Keybinds`
- clicking `Keybinds` opens the keybind editor in a tab
- the editor shows a row for every host-owned shortcut
- each row shows both the current binding and the default binding
- the tab closes like any other tab
- `Settings` shows the same editor on its `Keybindings` page

### 7. Remap From The Editor

Launch Limux, open terminal `Keybinds`, click the `Split Right` binding, and press `Ctrl+H`.

Verify:

- the `Split Right` row updates to `Ctrl+H`
- `~/.config/limux/shortcuts.json` contains the `split_right` override
- `Ctrl+H` splits right immediately without restarting Limux
- `Ctrl+Alt+D` no longer splits right
- the pane header split-right tooltip now shows `Ctrl+H`

### 8. Editor Validation

Launch Limux, open terminal `Keybinds`, and try these invalid captures on any row:

- press only `Shift+H`
- press only `Ctrl`
- assign a combo already used by another shortcut, such as `Ctrl+Alt+N` or `Ctrl+Alt+Q`

Verify:

- the row shows an inline error
- the previous binding remains visible after the error
- no new instance opens, and Limux does not quit
- the running app keeps the old working shortcut

### 9. Unknown ID Handling

Create:

```json
{
  "shortcuts": {
    "toggle_sidebar": "<Ctrl><Alt>b",
    "not_a_real_shortcut": "<Ctrl>x"
  }
}
```

Restart and verify:

- Limux warns that the unknown ID was ignored
- `toggle_sidebar` still remaps correctly

### 10. Invalid JSON Fallback

Write invalid JSON:

```json
{ this is not valid json
```

Restart Limux from a terminal and verify:

- Limux prints a warning
- Limux falls back to defaults
- default shortcuts work again

### 11. Cmd Alias Policy

Create:

```json
{
  "shortcuts": {
    "browser_focus_location": "<Super>l"
  }
}
```

Restart Limux and verify:

- the keybind editor displays `Cmd+L`
- either the physical `Meta+L` or `Super+L` combination focuses the browser address bar

### 12. Editable Widget Bypass

Launch a browser tab and verify:

- `Ctrl+L` focuses the address bar when the page has focus
- `Ctrl+L` is not stolen once the address bar already has focus
- `Ctrl+R` reloads only when the page has focus
- `Ctrl+C` and `Ctrl+V` keep native copy and paste inside the address bar and browser find field
- sidebar rename entries keep native text-editing behavior for `Ctrl+C` and `Ctrl+V`

### 13. Focused Surface Dispatch

Verify with a terminal tab focused:

- `Ctrl+Alt+F` opens terminal search
- `Ctrl+Alt+G` and `Ctrl+Alt+Shift+G` move through terminal search results
- `Ctrl+Alt+E` uses the current terminal selection for search
- `Ctrl+Alt+K`, `Ctrl+Shift+C`, `Ctrl+Shift+V`, `Ctrl+Alt++`, `Ctrl+Alt+-`, and `Ctrl+Alt+Shift+0` affect only the terminal

Verify with a browser tab focused:

- `Ctrl+Alt+F` opens the browser find bar
- `Ctrl+Alt+G` and `Ctrl+Alt+Shift+G` move through browser find results
- `Ctrl+Alt+Shift+F` hides the browser find bar and returns focus to the page
- `Ctrl+Alt+E` seeds browser find from the current DOM selection when page text is selected
- terminal shortcuts like `Ctrl+Alt+K` do not fire on the browser

### 14. Browser Navigation And Devtools

Verify with a browser tab focused:

- `Ctrl+[` navigates back
- `Ctrl+]` navigates forward
- `Ctrl+R` reloads
- `Ctrl+Alt+I` opens Web Inspector
- `Ctrl+Alt+C` also opens Web Inspector because WebKitGTK does not expose a console-only shortcut target
- `Ctrl+Shift+L` opens a new split with a browser tab

## Good Test Cases

If you only want a short smoke test, do these three:

1. Remap `toggle_sidebar` to `<Ctrl><Alt>b`
2. Unbind `split_right`
3. Remap `new_terminal` to `<Ctrl><Alt>y`

That covers:

- GTK accelerators
- capture-phase dispatch
- visible tooltips
- old-binding disablement
- pass-through after unbind

## Relevant Source Files

- `rust/limux-host-linux/src/shortcut_config.rs`
- `rust/limux-host-linux/src/main.rs`
- `rust/limux-host-linux/src/window.rs`
- `rust/limux-host-linux/src/pane.rs`
