# Keyboard shortcuts

Most defaults use `Ctrl+Alt`, so plain terminal `Ctrl` editing keys pass through to your program. Fullscreen is `F11`.

Change shortcuts in **Settings > Keybindings**, or in `~/.config/limux/shortcuts.json` (see [shortcut-remap-testing.md](shortcut-remap-testing.md) for the format). Custom shortcuts may also use `Cmd`, which matches the Linux `Meta` or `Super` modifier. `Opt` maps to `Alt`.

## Workspaces

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+Shift+N` | New workspace (uses the configured folder prompt) |
| `Ctrl+Alt+Shift+W` | Close workspace |
| `Ctrl+Alt+1-9` | Switch to workspace by number |
| `Ctrl+Alt+PageDown/Up` | Next or previous workspace |
| `Ctrl+Alt+O` | Most recent workspace, back to the pane you left |
| `Ctrl+Alt+U` | Next unread terminal, across workspaces and collapsed groups |
| `Ctrl+Alt+Shift+U` | Previous unread terminal |
| `Ctrl+Alt+M` | Toggle sidebar |
| `Ctrl+Alt+Shift+M` | Toggle top bar |

## Panes and tabs

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+D` | Split right |
| `Ctrl+Alt+Shift+D` | Split down |
| `Ctrl+Alt+Arrow` | Focus pane in direction |
| `Ctrl+Shift+Z` | Toggle zoom of the focused pane |
| `Ctrl+Alt+W` | Close focused pane |
| `Ctrl+Alt+T` | New terminal tab |
| `Ctrl+Shift+T` | New terminal tab in the focused pane |
| `Ctrl+Alt+Shift+Left/Right` | Cycle tabs in the focused pane |
| `Ctrl+Shift+W` | Close focused tab |

## Terminal

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+C` | Copy selection |
| `Ctrl+Shift+V` | Paste (text, or a copied image as a file) |
| `Ctrl+Alt+K` | Clear scrollback |
| `Ctrl+Alt++` | Increase font size |
| `Ctrl+Alt+-` | Decrease font size |
| `Ctrl+Alt+Shift+0` | Reset font size |

## Find

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+F` | Open find in the focused terminal or browser |
| `Ctrl+Alt+G` | Find next |
| `Ctrl+Alt+Shift+G` | Find previous |
| `Ctrl+Alt+Shift+F` | Hide find |
| `Ctrl+Alt+E` | Use selection for find |

## Browser

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+L` | Open the focused browser page in a new split |
| `Ctrl+L` | Focus the address bar |
| `Ctrl+[` | Back |
| `Ctrl+]` | Forward |
| `Ctrl+R` | Reload |
| `Ctrl+Alt+I` | Open Web Inspector |
| `Ctrl+Alt+C` | Open Web Inspector (WebKitGTK cannot open only the console) |

## App

| Shortcut | Action |
|---|---|
| `F11` | Toggle fullscreen |
| `Ctrl+Alt+N` | Open a new Limux instance |
| `Ctrl+Alt+Q` | Quit Limux |
