# Using Limux

## Workspaces

A workspace is one project: a folder, a split layout, tabs, and a name.

- **+** opens a workspace in your home folder. The folder icon beside it opens one in a folder you choose.
- To make **+** ask for a folder each time, turn on **Settings > General > Ask for folder on new workspace**.
- Right-click a workspace in the sidebar for rename, color, group, and autostart.
- Drag workspaces to reorder them or move them into a group. Use the star to pin favorites.

Limux saves workspaces, layouts, and tabs, and restores them the next time it starts. When you switch back to a workspace, you return to the pane you last used there, with its active tab.

## Finding the terminal that needs you

A notification marks the tab, the workspace row, and the top bar. `Ctrl+Alt+U` goes to the next unread terminal and `Ctrl+Alt+Shift+U` to the previous one. They work across workspaces and collapsed groups.

Background tabs show a spinner for five seconds after they receive output, including animated titles from tools such as Codex and output in renamed tabs. Cursor blinking and scrolling alone do not count as output. Viewing a tab clears its activity. Leaving it does not start the spinner again until new output arrives. A notification dot replaces the spinner until you read the notification.

In **Settings > General** you can turn off **Tab activity indicators** or set **Tab activity timeout** from 1 to 60 seconds.

Any script can raise a notification:

```bash
limux notify "Build done"
```

## When a terminal's process ends

A terminal you are looking at closes when its process ends, as in any terminal. The workspace closes with its last terminal unless **Keep workspace open** is on.

If the process ends while you are not looking (another workspace, a background tab, or another window in front), Limux keeps the tab, its output, and the workspace. It marks them unread and shows **Restart** and **Close**.

- If the process failed (a non-zero exit or a signal), Limux also raises a desktop notification when notifications are on. A clean exit is only marked in the sidebar.
- **Restart** runs the same command again, including the workspace's autostart command.
- A command that fails right after it starts is kept the same way, so you can read its error.

## Workspace autostart

Right-click a workspace and select **Set Autostart…** to run a command whenever Limux creates a terminal in that workspace. Limux saves the command with the workspace and runs it inside the terminal, so interactive commands such as `ssh user@server` work without changes to `.bashrc`. To turn it off, open **Edit Autostart…** and save an empty command.

Limux writes the command to a private, self-deleting `/bin/sh` script and asks the interactive shell to source it. Changes such as `cd` and `export` stay in that shell.

Autostart runs only when both `command` and `initial-command` in Ghostty's final configuration launch a recognized POSIX-compatible shell with no arguments other than interactive or login flags. Other programs, such as `command = direct:/usr/bin/vim`, and non-interactive wrappers, such as `command = direct:/bin/bash -lc "exec /usr/bin/vim"`, skip autostart. Limux does not send keystrokes to a program you launched.

## Pasting images

Paste (`Ctrl+Shift+V`, or **Paste** in the terminal menu) can turn a copied image into a file.

- **Local terminal:** Limux saves a private PNG at `/tmp/limux-paste.<random>/image.png` and inserts that path plus a space. A local Codex or Claude Code composer can attach it. The file stays until you delete it or the system cleans `/tmp`.
- **Persistent SSH tab:** Limux uploads the image to that host and inserts the remote path.

Paste does not press Enter. Limux does not detect an `ssh` or container command that you type yourself. Use a persistent SSH tab when the file has to be on another machine. The Flatpak build writes a local image on the host. See [ssh.md](ssh.md#clipboard-images) for paths, lifetime, and limits.

## Bring the window forward

Bind a desktop shortcut to this command to bring the running Limux window forward. It does not open another instance or change the selected workspace or tab.

```bash
limux activate
```

If your desktop does not include the Limux install directory in `PATH`, use the full path to the executable.

The command uses the control socket. To target one instance:

```bash
limux --socket /path/to/instance.sock activate
```

An explicit `--socket` takes precedence over `LIMUX_SOCKET`, `LIMUX_SOCKET_PATH`, and the default runtime socket. If no instance listens there, the command fails with a socket connection error. It never launches an instance. Plain `limux` still launches the app, and you can run independent instances.

Limux forwards `XDG_ACTIVATION_TOKEN`, or `DESKTOP_STARTUP_ID` when there is no token, to GTK. Your compositor decides whether to grant focus, especially on Wayland, where a global shortcut may need a valid activation token. Success means that Limux asked to be presented, not that it received focus.

Socket authentication still applies. The default `localUser` policy permits a shortcut that the same user runs. `LIMUX_SOCKET_MODE=limuxOnly` rejects commands from outside that Limux process's descendants, which includes desktop shortcuts.
