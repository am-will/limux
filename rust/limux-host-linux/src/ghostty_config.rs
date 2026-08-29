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

pub(crate) fn terminal_command_accepts_shell_input(configured_command: Option<&str>) -> bool {
    match configured_command {
        Some(command) => command_launches_interactive_shell(command),
        None => std::env::var_os("SHELL")
            .map(|shell| command_launches_interactive_shell(&shell.to_string_lossy()))
            // Ghostty falls back to the user's passwd shell when SHELL is
            // absent, so the default command remains an interactive shell.
            .unwrap_or(true),
    }
}

fn command_launches_interactive_shell(command: &str) -> bool {
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

    let supported_shell = matches!(
        name,
        "sh" | "bash" | "dash" | "zsh" | "fish" | "ksh" | "mksh" | "nu" | "elvish" | "xonsh"
    );

    supported_shell && arguments.all(is_interactive_shell_argument)
}

fn is_interactive_shell_argument(argument: &str) -> bool {
    matches!(argument, "-i" | "--interactive" | "-l" | "--login")
        || argument.strip_prefix('-').is_some_and(|flags| {
            !flags.is_empty() && flags.chars().all(|flag| matches!(flag, 'i' | 'l'))
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

#[cfg(test)]
mod tests {
    use super::{
        command_launches_interactive_shell, read_ghostty_value,
        terminal_command_accepts_shell_input,
    };

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
            "",
        ] {
            assert!(!command_launches_interactive_shell(command), "{command}");
        }
    }

    #[test]
    fn terminal_command_classification_accepts_supported_shells() {
        for command in [
            "/bin/bash",
            "direct:/usr/bin/zsh -l",
            "direct:/usr/bin/zsh -il",
            "shell:fish --login",
            "'sh'",
            "nu",
        ] {
            assert!(command_launches_interactive_shell(command), "{command}");
        }
    }

    #[test]
    fn absent_command_uses_shell_fallback() {
        assert!(terminal_command_accepts_shell_input(None));
    }
}
