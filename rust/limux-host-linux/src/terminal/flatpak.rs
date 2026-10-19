use std::cell::RefCell;
use std::ffi::{CString, NulError};

#[derive(Default)]
struct Commands {
    command: Option<String>,
    initial_command: Option<String>,
    surface_created: bool,
}

impl Commands {
    fn selected<'a>(&'a self, startup_command: Option<&'a str>) -> Option<&'a str> {
        startup_command.or_else(|| {
            if self.surface_created {
                self.command.as_deref()
            } else {
                self.initial_command.as_deref().or(self.command.as_deref())
            }
        })
    }
}

thread_local! {
    static COMMANDS: RefCell<Commands> = RefCell::default();
}

pub(super) fn enabled() -> bool {
    std::env::var_os("FLATPAK_ID").is_some_and(|id| id == crate::APP_ID)
}

pub(super) fn load_config(contents: &str) {
    COMMANDS.with_borrow_mut(|commands| {
        commands.command = crate::ghostty_config::read_ghostty_value(contents, "command")
            .filter(|command| !command.is_empty());
        commands.initial_command =
            crate::ghostty_config::read_ghostty_value(contents, "initial-command")
                .filter(|command| !command.is_empty());
    });
}

pub(super) fn surface_created() {
    COMMANDS.with_borrow_mut(|commands| commands.surface_created = true);
}

pub(super) fn argv(
    startup_command: Option<&str>,
    extra_env: &[(String, String)],
) -> Result<Vec<CString>, NulError> {
    COMMANDS.with_borrow(|commands| {
        let mut names = vec!["TERM".to_string(), "COLORTERM".to_string()];
        names.extend(
            std::env::vars_os()
                .filter_map(|(key, _)| key.into_string().ok())
                .filter(|key| key.starts_with("LIMUX_")),
        );
        names.extend(extra_env.iter().map(|(key, _)| key.clone()));
        build_argv(commands.selected(startup_command), names)
            .into_iter()
            .map(CString::new)
            .collect()
    })
}

fn build_argv(command: Option<&str>, mut names: Vec<String>) -> Vec<String> {
    names.sort_unstable();
    names.dedup();
    let mut argv = vec![
        "/app/bin/host-spawn".to_string(),
        "--pty".to_string(),
        format!("--env={}", names.join(",")),
        // Ghostty's bundled terminfo lives inside /app. Use the portable
        // host entry, without requiring a native Ghostty installation.
        "/usr/bin/env".to_string(),
        "TERM=xterm-256color".to_string(),
    ];
    match command.map(|command| command.trim_matches(' ')) {
        Some(command) if command.starts_with("direct:") => {
            // Ghostty's direct: syntax splits on spaces without shell expansion.
            argv.extend(
                command[7..]
                    .trim_matches(' ')
                    .split(' ')
                    .map(str::to_string),
            );
        }
        Some(command) => argv.extend([
            "/bin/sh".to_string(),
            "-c".to_string(),
            command
                .strip_prefix("shell:")
                .unwrap_or(command)
                .to_string(),
        ]),
        None => argv.extend([
            "/bin/sh".to_string(),
            "-c".to_string(),
            // SHELL comes from the host session, not the sandbox's /bin/sh.
            "exec \"${SHELL:-/bin/sh}\" -l".to_string(),
        ]),
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::{build_argv, Commands};

    #[test]
    fn initial_command_applies_only_before_the_first_surface() {
        let mut commands = Commands {
            command: Some("bash".into()),
            initial_command: Some("nvim".into()),
            surface_created: false,
        };
        assert_eq!(commands.selected(None), Some("nvim"));
        assert_eq!(commands.selected(Some("ssh host")), Some("ssh host"));
        commands.surface_created = true;
        assert_eq!(commands.selected(None), Some("bash"));
        assert_eq!(commands.selected(Some("ssh host")), Some("ssh host"));
    }

    #[test]
    fn shell_command_remains_one_argument_without_sandbox_expansion() {
        let command = "printf '%s\\n' \"$HOME\" 'a b' \"it's fine\"\nprintf done";
        let argv = build_argv(Some(&format!("shell:{command}")), vec![]);
        assert_eq!(&argv[5..], ["/bin/sh", "-c", command]);
    }

    #[test]
    fn direct_command_preserves_ghostty_literal_space_splitting() {
        let argv = build_argv(Some("direct:printf  $HOME *.rs 'quoted'"), vec![]);
        assert_eq!(&argv[5..], ["printf", "", "$HOME", "*.rs", "'quoted'"]);
    }

    #[test]
    fn terminal_environment_is_forwarded_without_copying_sandbox_path() {
        let argv = build_argv(
            None,
            vec!["TERM".into(), "LIMUX_SOCKET".into(), "LIMUX_SOCKET".into()],
        );
        assert_eq!(argv[2], "--env=LIMUX_SOCKET,TERM");
        assert_eq!(&argv[3..5], ["/usr/bin/env", "TERM=xterm-256color"]);
        assert_eq!(
            &argv[5..],
            ["/bin/sh", "-c", "exec \"${SHELL:-/bin/sh}\" -l"]
        );
    }

    #[test]
    fn invalid_command_is_reported_instead_of_panicking() {
        assert!(super::argv(Some("printf\0bad"), &[]).is_err());
    }
}
