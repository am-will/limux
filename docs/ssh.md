# SSH workspace launch

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
forwarding. Limux does not change authentication or host-key checking policy.

The terminal launches `env TERM=xterm-256color ssh -t [ -p PORT ] -- DESTINATION`,
with each argument quoted for Ghostty's command-string API. Destination input is
validated before launch; option prefixes, whitespace, shell metacharacters,
control characters, and invalid ports are rejected. The standard terminal type
avoids requiring Ghostty terminfo on the remote host. No remote command or setup
script is appended.

This is the first slice of [the SSH proposal](https://github.com/am-will/limux/issues/135):

1. Host discovery and explicit workspace launch (this change).
2. Persistent SSH tabs, remote tmux sessions, and reconnect.
3. Image transfer and remote clipboard integration.
4. Notifications and opt-in helper provisioning.

Closing the connection ends the SSH client normally. Workspace layout and title
follow ordinary Limux persistence, but the SSH launch command is not saved.
Restoring that workspace, creating another tab, or splitting a pane opens an
ordinary local terminal. Use **Connect via SSH…** for another connection.
Host-book editing, automatic reconnect, tmux setup, file transfer, and helper
installation are not part of this slice.

## Validation

`./scripts/check.sh` includes focused host parsing, destination validation, and
command-construction tests. `LIMUX_SMOKE_PROFILE=debug ./scripts/xvfb-smoke-test.sh`
also runs the live SSH launch regression under a private headless Weston session.
That regression uses an executable SSH fixture to check the actual argv, portable
TERM, and PTY, and verifies that new tabs and restored layouts do not launch SSH.
It needs no SSH server, credentials, or access to a real host.

For a manual check, connect to a disposable host, confirm the normal OpenSSH
authentication prompt and interactive shell, exit the shell, and verify ordinary
local workspace behavior. Test configured aliases with non-default ports and
jump hosts using your own SSH configuration.
