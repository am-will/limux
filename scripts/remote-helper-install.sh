#!/bin/sh
# Invoked by the explicit SSH helper dialog. Binary arrives on stdin; arguments
# are a locally computed SHA256, ELF architecture, agent, and exact byte count.
set -eu
umask 077
hash=$1 arch=$2 agent=$3 size=$4
case "$hash" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#hash}" = 64 ] || exit 2
case "$size" in ''|*[!0-9]*) exit 2;; esac
[ "$size" -gt 0 ] && [ "$size" -le 67108864 ] || exit 2
case "$arch" in x86_64|aarch64) ;; *) exit 2;; esac
[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = "$arch" ] || {
    echo 'Helper architecture does not match this Linux host' >&2; exit 1;
}
case "$agent" in
    codex) config_dir=${CODEX_HOME:-"$HOME/.codex"}; config="$config_dir/hooks.json"; export CODEX_HOME="$config_dir";;
    claude) config_dir=${CLAUDE_CONFIG_DIR:-"$HOME/.claude"}; config="$config_dir/settings.json"; export CLAUDE_CONFIG_DIR="$config_dir";;
    gemini) config_dir="$HOME/.gemini"; config="$config_dir/settings.json";;
    *) exit 2;;
esac
case "$config_dir" in /*) ;; *) echo 'Agent config directory must be absolute' >&2; exit 1;; esac
# Do not replace symlinks or silently redirect the installation elsewhere.
for dir in "$HOME/.local" "$HOME/.local/share" "$HOME/.local/share/limux" "$HOME/.local/share/limux/ssh-helpers"; do
    [ ! -L "$dir" ] || { echo "Refusing symlink: $dir" >&2; exit 1; }
    mkdir -p "$dir"
    [ "$(stat -c %u "$dir")" = "$(id -u)" ] && [ "$((0$(stat -c %a "$dir") & 022))" -eq 0 ] || {
        echo "Helper parent must be owned by this user and not group/world-writable: $dir" >&2; exit 1;
    }
done
base="$HOME/.local/share/limux/ssh-helpers"
[ ! -L "$base/.install.lock" ] && [ ! -L "$config_dir" ] && [ ! -L "$config" ] || {
    echo 'Refusing symlinked install lock or agent configuration' >&2; exit 1;
}
exec 9>>"$base/.install.lock"
flock -n 9 || { echo 'Another helper installation is running' >&2; exit 1; }
stage=$(mktemp -d "$base/.install.XXXXXXXXXX")
trap 'if [ -d "$stage" ]; then rm -f "$stage/limux-cli" "$stage/help"; rmdir "$stage"; fi' 0
trap 'exit 1' HUP INT TERM
cat > "$stage/limux-cli"
[ "$(wc -c < "$stage/limux-cli")" -eq "$size" ] || { echo 'Incomplete helper upload' >&2; exit 1; }
printf '%s  %s\n' "$hash" "$stage/limux-cli" | sha256sum -c - >/dev/null
chmod 700 "$stage/limux-cli"
"$stage/limux-cli" --help > "$stage/help"
grep -Fq 'hooks setup [agent] [--osc]' "$stage/help" || {
    echo 'Helper cannot run or does not support OSC hook setup' >&2; exit 1;
}
rm "$stage/help"
destination="$base/$hash"
if [ -e "$destination" ] || [ -L "$destination" ]; then
    [ ! -L "$destination" ] && [ ! -L "$destination/limux-cli" ] || exit 1
    for path in "$destination" "$destination/limux-cli"; do
        [ "$(stat -c %u "$path")" = "$(id -u)" ] && [ "$((0$(stat -c %a "$path") & 022))" -eq 0 ] || {
            echo "Existing helper path has unsafe owner/permissions: $path" >&2; exit 1;
        }
    done
    printf '%s  %s\n' "$hash" "$destination/limux-cli" | sha256sum -c - >/dev/null
else
    # Publish only a complete, verified executable. Never replace a user's CLI.
    mv "$stage" "$destination"
fi
# Back up before the canonical installer merges this agent's hooks. Existing
# Limux hooks for this agent are replaced; unrelated hooks are preserved.
backup=$(mktemp -d "$base/backup-$agent.XXXXXXXXXX")
if [ -e "$config" ]; then
    cp "$config" "$backup/config.json"
else
    : > "$backup/config-was-absent"
fi
printf '%s\n' "$config" > "$backup/config-path"
echo "Agent configuration backup: $backup" >&2
"$destination/limux-cli" hooks setup "$agent" --osc >&2
echo "Helper executable: $destination/limux-cli" >&2
printf 'LIMUX_HELPER_INSTALLED\n'
