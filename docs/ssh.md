# SSH workspaces

Choose **Connect via SSH…** at the bottom of the sidebar. Select an alias
discovered in `~/.ssh/config`, or enter a hostname, IPv6 address (without
brackets), or `user@hostname`. An optional port overrides the SSH configuration.
Click **Connect** to open a new workspace. Authentication and host-key prompts
appear in its terminal, using the installed OpenSSH client.

Discovery reads only literal aliases from top-level `Host` directives. It handles
multiple aliases, quoted patterns, comments, case-insensitive directive names,
and `Host=alias` syntax. Wildcards, negated patterns, malformed aliases, and
duplicates are omitted. `Include` files are not traversed: enter their aliases
manually. Unicode names should be entered in their ASCII/Punycode form.

The selected alias is passed unchanged to OpenSSH. Limux does not attempt to
resolve `HostName`, `IdentityFile`, `ProxyJump`, `Match`, tokens, or option
precedence itself. It does not invoke `ssh -G` during discovery, since evaluating
SSH configuration may execute `Match exec`. Connecting explicitly uses the
user's existing SSH configuration, including any configured commands or
forwarding. Interactive connections use OpenSSH's configured authentication and host-key
policies.

A one-shot connection launches `env TERM=xterm-256color ssh -t [ -p PORT ] -- DESTINATION`,
with each argument quoted for Ghostty's command-string API. Destination input is
validated before launch; option prefixes, whitespace, shell metacharacters,
control characters, and invalid ports are rejected. The standard terminal type
avoids requiring Ghostty terminfo on the remote host. No remote command or setup
script is appended.

## Persistent sessions and reconnect

Enable **Keep remote sessions with tmux and restore on startup** in the SSH
dialog to save the connection per terminal tab. This is opt-in; leaving it off
retains the one-shot behavior described above. Without persistence, restored
workspaces, new tabs, and splits open local terminals.

Persistent connections require an existing `tmux` executable on the remote
host. Limux starts or attaches to `tmux new-session -A -s limux-<uuid>`. Each tab
has its own session name. Restoring a saved tab keeps that name; creating a tab
or splitting an active persistent SSH terminal inherits its destination, port,
and reconnect preference, but allocates a new session. Tabs in inactive
workspaces start when GTK first creates their terminal surfaces.

Closing a tab or Limux disconnects the local client and leaves remote tmux
sessions running. A saved tab can reattach after restarting Limux. Closing a
tab removes its saved connection; Limux does not kill or clean up that remote
session. End the remote shell normally, or manage detached sessions with tmux.
A remote host reboot or tmux-server exit destroys the remote processes; a later
connection creates a fresh session under the saved name.

**Reconnect automatically after SSH errors** is enabled by default when opting
into persistence and can be disabled independently. OpenSSH exit status 255
schedules another attempt after 1, 2, 4, 8, 16, then at most 32 seconds. Retries
continue without deleting the saved tab. A connection lasting at least a minute
resets the delay. This status also covers authentication and host-key failures;
Limux cannot distinguish them from network errors using the exit status alone.
Close the tab to stop retries. Authentication prompts remain in the terminal.

A normal shell exit or tmux detach (status 0) closes the tab without reconnecting.
Other nonzero statuses, including missing tmux, retain the exited terminal and
saved target without retrying. Their error output remains visible; restart
Limux to retry after correcting the problem. A tab tooltip indicates an SSH
error or a scheduled reconnect. Automatic retries preserve tab names, pins,
identity, and selection, including after moving a tab to another pane.

## Execution and persistence boundaries

Persistence records only the validated destination, optional port, generated
tmux session identity, and reconnect preference in Limux's normal session file.
It stores no passwords, keys, shell commands, or resolved SSH configuration.
Saved values are validated again when loaded. Both local and remote command
arguments are shell-quoted; session names must be canonical Limux UUID names.
Remote working-directory announcements are not used as local launch directories.

Opting in authorizes reconnecting that target on restoration and, if enabled,
after SSH errors. Each attempt uses the current OpenSSH configuration, which
may execute configured `Match exec`, `ProxyCommand`, or forwarding directives.
The remote login shell resolves `tmux` through its PATH, and tmux uses the remote
account's normal configuration and shell. Limux neither installs nor verifies
those remote programs. SSH authentication and host-key verification remain
OpenSSH's responsibility; their policies are not weakened.

Persistent launches add `ServerAliveInterval=15`, `ServerAliveCountMax=3`, and
`ConnectTimeout=10`. They do not install helpers, change tmux global options,
enable passthrough, or establish a separate SSH control socket. User-configured
SSH multiplexing and forwarding still apply. Agent notification
forwarding and helper provisioning remain a later slice of
[the SSH proposal](https://github.com/am-will/limux/issues/135).

## Validation

`./scripts/check.sh` covers host parsing, destination validation, session
serialization, rejection of unsafe saved values, both shell argument boundaries,
and the retry policy. Tests live in the existing inline Rust test modules.

`LIMUX_SMOKE_PROFILE=debug ./scripts/xvfb-smoke-test.sh` runs SSH regressions
under a private headless Weston session. Executable SSH fixtures check actual
argv, TERM, and PTY; transient launch remains non-persistent. The persistent
fixture exercises repeated errors, background reconnect, separate tab/split
identities, names/pins, moving during backoff, JSON restoration, cancellation on
close, intentional exit, non-retry errors, disabled retries, and remote cwd
announcements. These tests need no SSH server or credentials.

For an additional live check, use a disposable SSH host with tmux. Record the
remote shell PID, interrupt the network connection, and verify reattachment to
the same PID. Restart Limux and verify each tab's session identity. Confirm
normal detach/exit stops retries, missing tmux leaves an error, and closing a
tab during an outage stops new SSH processes. Do not disrupt shared SSH masters
or other users' tmux sessions when simulating a lost connection.

## Clipboard image transfer

In a **persistent SSH tab**, copy an image to the desktop clipboard and use
Limux's **Paste** shortcut (default Ctrl+Shift+V) or the terminal context menu's
**Paste** action. Limux encodes the image as PNG, uploads it asynchronously to
the tab's saved SSH destination, then pastes the remote filename followed by a
space. Bracketed paste is preserved; no Enter key or agent prompt is submitted.
Agents such as Codex can recognize the path as an image attachment. Ordinary
text continues through Ghostty's existing clipboard path.

This requires actual clipboard image data, not a copied filename, file-manager
URI, or drag-and-drop. Local terminals, one-shot SSH tabs, and nested `ssh`
commands have no supported upload destination. In a nested SSH/container
session, the path still belongs to the persistent tab's original SSH host;
Limux does not infer or follow nested destinations. Use the agent on that host
with access to its `/tmp`. Ghostty-only custom paste bindings do not invoke this
Limux action. Terminal-initiated OSC 52 clipboard reads cannot start an upload;
existing clipboard consent handling remains in effect.

Each paste starts a separate non-PTY `ssh` command, preserving the alias and
explicit port. It uses current SSH configuration, including user-configured
multiplexing, ProxyJump, ProxyCommand, Match commands, and forwarding. The upload
requires noninteractive authentication (keys/agent or an existing multiplexed
connection). `BatchMode=yes`, `SSH_ASKPASS_REQUIRE=never`, and
`StrictHostKeyChecking=yes` prevent background password/host-key prompts and
reject unknown or changed host keys. Password-only interactive sessions may
connect normally but fail to upload. No keys or passwords are saved, and no
SSH master is created by Limux. Flatpak uses the bundled host-spawn with
`--no-pty` to run host OpenSSH and preserve binary stdin.

The remote command runs a fixed `/bin/sh` script using `mktemp`, `cat`, `test`,
`printf`, `rm`, and `rmdir` from the remote account's environment. Those programs
and the SSH host are trusted; no helper is installed or downloaded. A private
`/tmp/limux-paste.XXXXXXXXXX` directory (mode 0700) contains `image.png` (0600).
PNG bytes are streamed on stdin, with no local staging file. Only a successful
SSH exit and an exact path matching this format permit terminal insertion;
remote output is never evaluated as a command.

Successful uploads remain on the remote host so agents can read them later.
Remove their directories when no longer needed, or let the host's temporary
file policy clean them up. The remote script attempts cleanup on failure and
signals; abrupt disconnection can still leave a partial or completed file.
Limux does not recursively clean remote directories on tab close.

Images are limited to 16,777,216 pixels and 32 MiB of encoded PNG. One upload
per terminal and four across the application may be pending. Uploads have a
30-second deadline, with a 45-second overall clipboard/paste deadline. Status
and errors appear in the terminal; SSH diagnostics go to the launch log. Failed
uploads insert nothing. Closing or reconnecting cancels the pending upload;
late callbacks cannot paste into a replacement terminal. Moving the same tab
between panes keeps the upload attached to that tab.

Upload argument construction, binary preservation, permissions, malformed or
oversized replies, nonzero exits, deadlines, cancellation, and failed-file
cleanup are covered by `./scripts/check.sh`. The maintained GUI smoke suite also
checks image delivery, bracketed paste, ordinary text, duplicate requests,
context-menu errors, moving a tab, and cancellation on reconnect or close.

An optional live Codex attachment test is ignored by default:
`terminal::image_paste::tests::live_ssh_agent_image_attachment`. Run it under a
private display with `LIMUX_IMAGE_TEST_COMMAND` set to a disposable localhost
SSH/tmux command launching an authenticated Codex composer. It uses strict,
noninteractive localhost SSH for the upload and checks image attachment plus
text copy/paste without submitting a model turn. Supply a private Codex home
and tmux server, and clean up their credentials/processes afterward. This test
requires local credentials and an SSH server; the maintained smoke suite does
not.
