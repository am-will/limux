use std::ffi::OsStr;
use std::path::Path;

use crate::terminal_font::{
    DEFAULT_CELL_METRIC_PERCENT, MAX_CELL_METRIC_PERCENT, MIN_CELL_METRIC_PERCENT,
};

const DEFAULT_FONT_SIZE: f32 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GhosttyFontDefaults {
    pub family: Option<String>,
    pub size: f32,
    pub cell_width_percent: u16,
    pub has_cell_width_modifier: bool,
    pub cell_height_percent: u16,
    pub has_cell_height_modifier: bool,
    pub use_ligatures: bool,
    pub anti_aliased: bool,
}

impl Default for GhosttyFontDefaults {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
            cell_width_percent: DEFAULT_CELL_METRIC_PERCENT,
            has_cell_width_modifier: false,
            cell_height_percent: DEFAULT_CELL_METRIC_PERCENT,
            has_cell_height_modifier: false,
            use_ligatures: true,
            anti_aliased: true,
        }
    }
}

impl GhosttyFontDefaults {
    pub(crate) fn from_config_contents(contents: &str) -> Self {
        let (cell_width_percent, has_cell_width_modifier) =
            parse_cell_metric_modifier(contents, "adjust-cell-width");
        let (cell_height_percent, has_cell_height_modifier) =
            parse_cell_metric_modifier(contents, "adjust-cell-height");
        Self {
            family: parse_primary_font_family(contents),
            size: parse_font_size(contents),
            cell_width_percent,
            has_cell_width_modifier,
            cell_height_percent,
            has_cell_height_modifier,
            use_ligatures: parse_use_ligatures(contents),
            anti_aliased: parse_anti_aliased(contents),
        }
    }
}

fn ghostty_config_contents_in(config_dir: &Path) -> Option<String> {
    // Match Ghostty's loadDefaultFiles precedence: legacy `config` first, then `config.ghostty`.
    let mut combined = String::new();
    for rel in ["ghostty/config", "ghostty/config.ghostty"] {
        let path = config_dir.join(rel);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str(&contents);
        }
    }
    (!combined.is_empty()).then_some(combined)
}

fn ghostty_config_contents() -> Option<String> {
    let config_dir = dirs::config_dir()?;
    ghostty_config_contents_in(&config_dir)
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

pub(crate) fn parse_font_size(contents: &str) -> f32 {
    read_ghostty_value(contents, "font-size")
        .and_then(|v| strip_optional_quotes(&v).trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(1.0, 255.0))
        .unwrap_or(DEFAULT_FONT_SIZE)
}

/// Read font-size from Ghostty default config files.
/// Returns the configured size in points (default: 12.0).
pub fn read_font_size() -> f32 {
    ghostty_config_contents()
        .map(|c| parse_font_size(&c))
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

pub(crate) fn parse_cell_metric_modifier(contents: &str, key: &str) -> (u16, bool) {
    let Some(raw) = read_ghostty_value(contents, key) else {
        return (DEFAULT_CELL_METRIC_PERCENT, false);
    };
    let unquoted = strip_optional_quotes(&raw).trim();
    if unquoted.is_empty() {
        return (DEFAULT_CELL_METRIC_PERCENT, false);
    }
    if let Some(percent_raw) = unquoted.strip_suffix('%') {
        if let Ok(delta) = percent_raw.trim().parse::<f64>() {
            if delta.is_finite() {
                let pct = (f64::from(DEFAULT_CELL_METRIC_PERCENT) + delta)
                    .round()
                    .clamp(
                        f64::from(MIN_CELL_METRIC_PERCENT),
                        f64::from(MAX_CELL_METRIC_PERCENT),
                    ) as u16;
                return (pct, delta.abs() >= f64::EPSILON);
            }
        }
        return (DEFAULT_CELL_METRIC_PERCENT, false);
    }
    if let Ok(pixels) = unquoted.parse::<i32>() {
        return (DEFAULT_CELL_METRIC_PERCENT, pixels != 0);
    }
    (DEFAULT_CELL_METRIC_PERCENT, false)
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

fn parse_font_feature_token(token: &str) -> Option<(String, bool)> {
    let mut rest = token.trim();
    if rest.is_empty() {
        return None;
    }

    let prefix_value = if let Some(after) = rest.strip_prefix('+') {
        rest = after;
        Some(true)
    } else if let Some(after) = rest.strip_prefix('-') {
        rest = after;
        Some(false)
    } else {
        None
    };

    let mut tag = String::with_capacity(4);
    let mut after_tag_idx = rest.len();
    for (idx, ch) in rest.char_indices() {
        if matches!(ch, '"' | '\'') {
            continue;
        }
        tag.push(ch);
        if tag.len() == 4 {
            after_tag_idx = idx + ch.len_utf8();
            break;
        }
    }
    if tag.len() != 4 {
        return None;
    }

    let mut tail = rest[after_tag_idx..].trim_start_matches([' ', '\t', '"', '\'']);
    if let Some(after_eq) = tail.strip_prefix('=') {
        if prefix_value.is_some() {
            return None;
        }
        tail = after_eq.trim_start_matches([' ', '\t']);
    }
    let tail = tail.trim_end_matches([' ', '\t']);

    let enabled = if tail.is_empty() {
        prefix_value.unwrap_or(true)
    } else if prefix_value.is_some() {
        return None;
    } else if tail.eq_ignore_ascii_case("on") {
        true
    } else if tail.eq_ignore_ascii_case("off") {
        false
    } else if let Ok(n) = tail.parse::<u32>() {
        n != 0
    } else {
        return None;
    };

    Some((tag, enabled))
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
                for token in unquoted.split(',') {
                    if let Some((tag, is_enabled)) = parse_font_feature_token(token) {
                        if matches!(tag.as_str(), "calt" | "liga") {
                            enabled = is_enabled;
                        }
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

pub(crate) fn read_font_defaults() -> GhosttyFontDefaults {
    ghostty_config_contents()
        .map(|c| GhosttyFontDefaults::from_config_contents(&c))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        classify_interactive_shell, ghostty_config_contents_in, parse_anti_aliased,
        parse_cell_metric_modifier, parse_primary_font_family, parse_use_ligatures,
        read_ghostty_value, terminal_command_autostart_shell_with_fallback,
        terminal_commands_autostart_shell, AutostartShellKind, GhosttyFontDefaults,
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

        // Ghostty CSS-compatible font-feature forms.
        assert!(!parse_use_ligatures("font-feature = \"calt\" = 0\n"));
        assert!(!parse_use_ligatures("font-feature = calt = 0\n"));
        assert!(!parse_use_ligatures("font-feature = 'liga' off\n"));
        assert!(!parse_use_ligatures("font-feature = \"calt\" 0\n"));
        assert!(parse_use_ligatures(
            "font-feature = \"calt\" = 0\nfont-feature = \"calt\" = 1\n"
        ));
        assert!(parse_use_ligatures("font-feature = -dlig\n"));

        // Serialized float percentages and pixel cell metric modifiers.
        assert_eq!(
            parse_cell_metric_modifier(
                "adjust-cell-width = 19.999999999999996%\n",
                "adjust-cell-width"
            ),
            (120, true)
        );
        assert_eq!(
            parse_cell_metric_modifier("adjust-cell-height = -8%\n", "adjust-cell-height"),
            (92, true)
        );
        assert_eq!(
            parse_cell_metric_modifier("adjust-cell-width = 0%\n", "adjust-cell-width"),
            (100, false)
        );
        assert_eq!(
            parse_cell_metric_modifier("adjust-cell-width = 4\n", "adjust-cell-width"),
            (100, true)
        );
        assert_eq!(
            parse_cell_metric_modifier("adjust-cell-width = \n", "adjust-cell-width"),
            (100, false)
        );
    }

    #[test]
    fn config_ghostty_overrides_legacy_config_in_fallback_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ghostty_dir = dir.path().join("ghostty");
        std::fs::create_dir_all(&ghostty_dir).expect("create ghostty dir");
        std::fs::write(
            ghostty_dir.join("config"),
            "font-family = DejaVu Sans Mono\nfont-size = 11\n",
        )
        .expect("write legacy config");
        std::fs::write(
            ghostty_dir.join("config.ghostty"),
            "font-family = \"\"\nfont-family = MesloLGS NF\nfont-size = 14\nadjust-cell-width = 20%\nfont-feature = \"calt\" = 0\nfreetype-load-flags = monochrome\n",
        )
        .expect("write config.ghostty");

        let combined = ghostty_config_contents_in(dir.path()).expect("combined config");
        let defaults = GhosttyFontDefaults::from_config_contents(&combined);
        assert_eq!(defaults.family.as_deref(), Some("MesloLGS NF"));
        assert_eq!(defaults.size, 14.0);
        assert_eq!(defaults.cell_width_percent, 120);
        assert!(defaults.has_cell_width_modifier);
        assert_eq!(defaults.cell_height_percent, 100);
        assert!(!defaults.has_cell_height_modifier);
        assert!(!defaults.use_ligatures);
        assert!(!defaults.anti_aliased);
    }
}
