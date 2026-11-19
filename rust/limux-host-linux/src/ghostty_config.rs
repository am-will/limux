use std::ffi::OsStr;

const DEFAULT_FONT_SIZE: f32 = 12.0;

fn ghostty_config_contents() -> Option<String> {
    let path = dirs::config_dir()
        .map(|d| d.join("ghostty/config"))
        .filter(|p| p.exists())?;
    std::fs::read_to_string(&path).ok()
}

pub(crate) fn read_ghostty_value(contents: &str, key: &str) -> Option<String> {
    for line in contents.lines().rev() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim();
            if let Some(value) = rest.strip_prefix('=') {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutostartShellKind {
    Posix,
    Fish,
    Nushell,
}

pub(crate) fn terminal_command_autostart_shell(
    configured_command: Option<&str>,
) -> Option<AutostartShellKind> {
    let fallback_shell = std::env::var_os("SHELL");
    terminal_command_autostart_shell_with_fallback(configured_command, fallback_shell.as_deref())
}

fn terminal_command_autostart_shell_with_fallback(
    configured_command: Option<&str>,
    fallback_shell: Option<&OsStr>,
) -> Option<AutostartShellKind> {
    match configured_command {
        Some(command) => classify_interactive_shell(command),
        None => match fallback_shell {
            Some(shell) => classify_interactive_shell(&shell.to_string_lossy()),
            // Ghostty falls back to the user's passwd shell when SHELL is
            // absent, so the default command remains a POSIX-compatible shell.
            None => Some(AutostartShellKind::Posix),
        },
    }
}

pub(crate) fn terminal_commands_autostart_shell(
    command: Option<&str>,
    initial_command: Option<&str>,
) -> Option<AutostartShellKind> {
    let primary = terminal_command_autostart_shell(command)?;
    match initial_command {
        Some(initial) => {
            let initial_kind = terminal_command_autostart_shell(Some(initial))?;
            (primary == initial_kind).then_some(primary)
        }
        None => Some(primary),
    }
}

fn classify_interactive_shell(command: &str) -> Option<AutostartShellKind> {
    let command = command.trim();
    let command = command
        .strip_prefix("direct:")
        .or_else(|| command.strip_prefix("shell:"))
        .unwrap_or(command)
        .trim_start();
    let mut arguments = command.split_ascii_whitespace();
    let executable = arguments
        .next()
        .unwrap_or_default()
        .trim_matches(['\'', '"']);
    let name = std::path::Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    match name {
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh" => arguments
            .all(is_interactive_posix_or_nu_argument)
            .then_some(AutostartShellKind::Posix),
        "fish" => arguments
            .all(is_interactive_fish_argument)
            .then_some(AutostartShellKind::Fish),
        "nu" => arguments
            .all(is_interactive_posix_or_nu_argument)
            .then_some(AutostartShellKind::Nushell),
        _ => None,
    }
}

fn is_interactive_posix_or_nu_argument(argument: &str) -> bool {
    matches!(argument, "-i" | "--interactive" | "-l" | "--login")
        || argument.strip_prefix('-').is_some_and(|flags| {
            !flags.is_empty() && flags.chars().all(|flag| matches!(flag, 'i' | 'l'))
        })
}

fn is_interactive_fish_argument(argument: &str) -> bool {
    matches!(
        argument,
        "-i" | "--interactive" | "-l" | "--login" | "-P" | "--private"
    ) || argument.strip_prefix('-').is_some_and(|flags| {
        !flags.is_empty()
            && !flags.starts_with('-')
            && flags.chars().all(|flag| matches!(flag, 'i' | 'l' | 'P'))
    })
}

/// Read background-opacity from the Ghostty config file.
/// Returns a value between 0.0 and 1.0 (default: 1.0 = fully opaque).
#[allow(dead_code)]
pub fn read_background_opacity() -> f64 {
    ghostty_config_contents()
        .and_then(|c| read_ghostty_value(&c, "background-opacity"))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Read font-size from the Ghostty config file.
/// Returns the configured size in points (default: 12.0).
pub fn read_font_size() -> f32 {
    ghostty_config_contents()
        .and_then(|c| read_ghostty_value(&c, "font-size"))
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(1.0, 255.0))
        .unwrap_or(DEFAULT_FONT_SIZE)
}

pub(crate) fn strip_optional_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

pub(crate) fn parse_primary_font_family(contents: &str) -> Option<String> {
    let mut families: Vec<String> = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("font-family") {
            let rest = rest.trim_start();
            if let Some(raw_val) = rest.strip_prefix('=') {
                let unquoted = strip_optional_quotes(raw_val).trim();
                if unquoted.is_empty() {
                    families.clear();
                } else if let Some(clean) = crate::terminal_font::sanitize_font_token(unquoted) {
                    families.push(clean);
                }
            }
        }
    }
    families.into_iter().next()
}

pub(crate) fn parse_use_ligatures(contents: &str) -> bool {
    let mut enabled = true;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("font-feature") {
            let rest = rest.trim_start();
            if let Some(raw_val) = rest.strip_prefix('=') {
                let unquoted = strip_optional_quotes(raw_val).trim();
                if unquoted.is_empty() {
                    enabled = true;
                    continue;
                }
                for token in unquoted.split(',').map(str::trim) {
                    let normalized = token.trim_matches(['"', '\'']).trim();
                    if matches!(
                        normalized,
                        "-calt" | "calt=0" | "calt off" | "-liga" | "liga=0" | "liga off"
                    ) {
                        enabled = false;
                    } else if matches!(
                        normalized,
                        "calt"
                            | "+calt"
                            | "calt=1"
                            | "calt on"
                            | "liga"
                            | "+liga"
                            | "liga=1"
                            | "liga on"
                    ) {
                        enabled = true;
                    }
                }
            }
        }
    }
    enabled
}

pub(crate) fn parse_anti_aliased(contents: &str) -> bool {
    let Some(raw) = read_ghostty_value(contents, "freetype-load-flags") else {
        return true;
    };
    let unquoted = strip_optional_quotes(&raw).trim();
    if unquoted.is_empty() || unquoted.eq_ignore_ascii_case("false") {
        return true;
    }
    if unquoted.eq_ignore_ascii_case("true") {
        return false;
    }
    let mut monochrome = false;
    for part in unquoted.split(',').map(str::trim) {
        if part.eq_ignore_ascii_case("monochrome") {
            monochrome = true;
        } else if part.eq_ignore_ascii_case("no-monochrome") {
            monochrome = false;
        }
    }
    !monochrome
}

pub fn read_font_family() -> Option<String> {
    ghostty_config_contents().and_then(|c| parse_primary_font_family(&c))
}

pub fn read_use_ligatures() -> bool {
    ghostty_config_contents()
        .map(|c| parse_use_ligatures(&c))
        .unwrap_or(true)
}

pub fn read_anti_aliased() -> bool {
    ghostty_config_contents()
        .map(|c| parse_anti_aliased(&c))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{
        classify_interactive_shell, parse_anti_aliased, parse_primary_font_family,
        parse_use_ligatures, read_ghostty_value, terminal_command_autostart_shell_with_fallback,
        terminal_commands_autostart_shell, AutostartShellKind,
    };
    use std::ffi::OsStr;

    #[test]
    fn last_ghostty_value_wins() {
        let contents = "command = /usr/bin/vim\ncommand = direct:/usr/bin/zsh -l\n";
        assert_eq!(
            read_ghostty_value(contents, "command").as_deref(),
            Some("direct:/usr/bin/zsh -l")
        );
    }

    #[test]
    fn terminal_command_classification_rejects_non_shell_commands() {
        for command in [
            "direct:/usr/bin/vim",
            "/usr/bin/nvim file.txt",
            "shell:tmux new-session",
            "direct:/bin/bash -lc exec-vim",
            "shell:/usr/bin/zsh -c nvim",
            "/bin/fish script.fish",
            "fish -c ls",
            "nu -c ls",
            "nu script.nu",
            "elvish",
            "xonsh",
            "",
        ] {
            assert_eq!(classify_interactive_shell(command), None, "{command}");
        }
    }

    #[test]
    fn terminal_command_classification_accepts_posix_fish_and_nushell() {
        for command in [
            "/bin/bash",
            "direct:/usr/bin/zsh -l",
            "direct:/usr/bin/zsh -il",
            "'sh'",
            "ksh -il",
        ] {
            assert_eq!(
                classify_interactive_shell(command),
                Some(AutostartShellKind::Posix),
                "{command}"
            );
        }

        for command in [
            "fish",
            "/usr/bin/fish --login",
            "direct:/usr/bin/fish -ilP",
            "fish --private --interactive",
        ] {
            assert_eq!(
                classify_interactive_shell(command),
                Some(AutostartShellKind::Fish),
                "{command}"
            );
        }

        for command in ["nu", "/usr/bin/nu -l", "direct:/usr/bin/nu --login -i"] {
            assert_eq!(
                classify_interactive_shell(command),
                Some(AutostartShellKind::Nushell),
                "{command}"
            );
        }
    }

    #[test]
    fn absent_command_uses_shell_fallback() {
        assert_eq!(
            terminal_command_autostart_shell_with_fallback(None, Some(OsStr::new("/bin/zsh"))),
            Some(AutostartShellKind::Posix)
        );
        assert_eq!(
            terminal_command_autostart_shell_with_fallback(None, Some(OsStr::new("/usr/bin/fish"))),
            Some(AutostartShellKind::Fish)
        );
        assert_eq!(
            terminal_command_autostart_shell_with_fallback(None, Some(OsStr::new("/usr/bin/nu"))),
            Some(AutostartShellKind::Nushell)
        );
        assert_eq!(
            terminal_command_autostart_shell_with_fallback(None, None),
            Some(AutostartShellKind::Posix)
        );
    }

    #[test]
    fn initial_command_must_match_autostart_shell_family() {
        assert_eq!(
            terminal_commands_autostart_shell(Some("direct:/bin/bash"), Some("direct:/bin/zsh -l")),
            Some(AutostartShellKind::Posix)
        );
        assert_eq!(
            terminal_commands_autostart_shell(
                Some("direct:/usr/bin/fish"),
                Some("direct:/usr/bin/fish -l")
            ),
            Some(AutostartShellKind::Fish)
        );
        assert_eq!(
            terminal_commands_autostart_shell(
                Some("direct:/usr/bin/fish"),
                Some("direct:/bin/bash")
            ),
            None
        );
        assert_eq!(
            terminal_commands_autostart_shell(
                Some("direct:/bin/bash"),
                Some("direct:/usr/bin/vim")
            ),
            None
        );
    }

    #[test]
    fn font_helpers_respect_repeatable_resets_ligature_toggles_and_monochrome() {
        let contents = "\
font-family = JetBrains Mono
font-family = \"\"
font-family = \"MesloLGS NF\"
font-family = Noto Color Emoji
font-feature = -calt, -liga
freetype-load-flags = light, monochrome
";
        assert_eq!(
            parse_primary_font_family(contents).as_deref(),
            Some("MesloLGS NF")
        );
        assert!(!parse_use_ligatures(contents));
        assert!(!parse_anti_aliased(contents));

        let reset = format!("{contents}font-family = \"\"\nfont-feature = +calt\nfreetype-load-flags = no-monochrome\n");
        assert_eq!(parse_primary_font_family(&reset), None);
        assert!(parse_use_ligatures(&reset));
        assert!(parse_anti_aliased(&reset));
    }
}
