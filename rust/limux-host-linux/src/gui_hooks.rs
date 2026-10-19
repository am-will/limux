//! GUI hooks: user commands that Limux runs when a workspace is created or
//! deleted from the GUI. Socket requests never fire a hook, so a hook can
//! create or close workspaces itself through the `limux` CLI.

use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiHookEvent {
    WorkspaceCreate,
    WorkspaceDelete,
}

impl GuiHookEvent {
    pub const ALL: [Self; 2] = [Self::WorkspaceCreate, Self::WorkspaceDelete];

    /// The name used by the CLI, the socket and `LIMUX_HOOK`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceCreate => "on-workspace-create",
            Self::WorkspaceDelete => "on-workspace-delete",
        }
    }

    /// The key under `hooks` in settings.json.
    pub fn config_key(self) -> &'static str {
        match self {
            Self::WorkspaceCreate => "on_workspace_create",
            Self::WorkspaceDelete => "on_workspace_delete",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::WorkspaceCreate => "On workspace create",
            Self::WorkspaceDelete => "On workspace delete",
        }
    }

    /// Accepts the CLI name or the settings key.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|event| raw == event.as_str() || raw == event.config_key())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiHook {
    /// A disabled hook keeps its settings but never runs.
    pub enabled: bool,
    /// A bash command line, e.g. `~/bin/new-ws.sh --force`.
    pub command: String,
    /// Run the command in a pop-up terminal instead of in the background.
    pub show_terminal: bool,
    /// Skip Limux's own create/delete; the hook is expected to do it.
    pub suppress_default: bool,
}

impl Default for GuiHook {
    fn default() -> Self {
        Self {
            enabled: true,
            command: String::new(),
            show_terminal: false,
            suppress_default: false,
        }
    }
}

impl GuiHook {
    pub fn is_configured(&self) -> bool {
        !self.command.trim().is_empty()
    }

    /// Whether the hook runs: it has a command and is enabled.
    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }

    fn from_value(value: &Value) -> Option<Self> {
        let hook = value.as_object()?;
        let flag = |key| hook.get(key).and_then(Value::as_bool).unwrap_or(false);
        Some(Self {
            // Hooks saved before this switch existed were all enabled.
            enabled: hook.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            command: hook.get("command").and_then(Value::as_str)?.to_string(),
            show_terminal: flag("show_terminal"),
            suppress_default: flag("suppress_default"),
        })
    }

    fn to_value(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "command": self.command,
            "show_terminal": self.show_terminal,
            "suppress_default": self.suppress_default,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuiHooksConfig {
    pub on_workspace_create: GuiHook,
    pub on_workspace_delete: GuiHook,
}

impl GuiHooksConfig {
    pub fn get(&self, event: GuiHookEvent) -> &GuiHook {
        match event {
            GuiHookEvent::WorkspaceCreate => &self.on_workspace_create,
            GuiHookEvent::WorkspaceDelete => &self.on_workspace_delete,
        }
    }

    pub fn get_mut(&mut self, event: GuiHookEvent) -> &mut GuiHook {
        match event {
            GuiHookEvent::WorkspaceCreate => &mut self.on_workspace_create,
            GuiHookEvent::WorkspaceDelete => &mut self.on_workspace_delete,
        }
    }

    /// The hook to run for `event`, if it has a command and is enabled.
    pub fn active(&self, event: GuiHookEvent) -> Option<&GuiHook> {
        Some(self.get(event)).filter(|hook| hook.is_active())
    }

    pub fn from_value(value: Option<&Value>) -> Self {
        let mut config = Self::default();
        if let Some(hooks) = value.and_then(Value::as_object) {
            for event in GuiHookEvent::ALL {
                if let Some(hook) = hooks.get(event.config_key()).and_then(GuiHook::from_value) {
                    *config.get_mut(event) = hook;
                }
            }
        }
        config
    }

    /// The `hooks` settings object; `None` when no hook is set at all.
    pub fn to_value(&self) -> Option<Value> {
        let hooks: Map<String, Value> = GuiHookEvent::ALL
            .into_iter()
            .filter(|event| *self.get(*event) != GuiHook::default())
            .map(|event| (event.config_key().to_string(), self.get(event).to_value()))
            .collect();
        (!hooks.is_empty()).then_some(Value::Object(hooks))
    }

    /// The socket payload for one hook.
    pub fn describe(&self, event: GuiHookEvent) -> Value {
        let hook = self.get(event);
        json!({
            "event": event.as_str(),
            "configured": hook.is_configured(),
            "enabled": hook.enabled,
            "command": hook.command,
            "show_terminal": hook.show_terminal,
            "suppress_default": hook.suppress_default,
        })
    }
}

/// A change to one hook; `None` fields keep their current value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuiHookUpdate {
    pub enabled: Option<bool>,
    pub command: Option<String>,
    pub show_terminal: Option<bool>,
    pub suppress_default: Option<bool>,
}

impl GuiHookUpdate {
    pub fn apply(&self, hook: &mut GuiHook) {
        if let Some(enabled) = self.enabled {
            hook.enabled = enabled;
        }
        if let Some(command) = &self.command {
            hook.command = command.trim().to_string();
        }
        if let Some(show_terminal) = self.show_terminal {
            hook.show_terminal = show_terminal;
        }
        if let Some(suppress_default) = self.suppress_default {
            hook.suppress_default = suppress_default;
        }
    }
}

/// What the hook is told about the event, as `LIMUX_*` variables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuiHookContext {
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub folder: Option<String>,
    pub group_id: Option<String>,
}

/// Variables the hook must not inherit from the host's own environment:
/// the CLI reads them to pick a default target.
pub const INHERITED_IDENTITY_VARS: &[&str] = &[
    "LIMUX_WORKSPACE_ID",
    "LIMUX_SURFACE_ID",
    "LIMUX_PANE_ID",
    "LIMUX_TAB_ID",
];

pub fn hook_environment(
    event: GuiHookEvent,
    hook: &GuiHook,
    context: &GuiHookContext,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("LIMUX_HOOK".to_string(), event.as_str().to_string()),
        (
            "LIMUX_HOOK_DEFAULT_ACTION".to_string(),
            if hook.suppress_default {
                "suppressed"
            } else {
                "run"
            }
            .to_string(),
        ),
    ];
    let fields = [
        ("LIMUX_WORKSPACE_ID", &context.workspace_id),
        ("LIMUX_WORKSPACE_NAME", &context.workspace_name),
        ("LIMUX_WORKSPACE_FOLDER", &context.folder),
        ("LIMUX_GROUP_ID", &context.group_id),
    ];
    for (name, value) in fields {
        if let Some(value) = value {
            env.push((name.to_string(), value.clone()));
        }
    }
    env
}

/// The Ghostty command for a pop-up hook terminal: run the hook in bash with
/// `unset` variables removed, then keep the output on screen until the user
/// presses Enter. Ghostty's C API hands the whole string to `/bin/sh -c`; it
/// does not parse the `shell:`/`direct:` prefixes of its config file.
pub fn terminal_command(command: &str, unset: &[&str]) -> String {
    let unset = if unset.is_empty() {
        String::new()
    } else {
        format!("unset {}\n", unset.join(" "))
    };
    let script = format!(
        "{unset}{command}\n\
         limux_hook_status=$?\n\
         printf '\\n[%s exited with status %d. Press Enter to close.]' \"$LIMUX_HOOK\" \"$limux_hook_status\"\n\
         read -r _\n"
    );
    format!("bash -c {}", single_quote(&script))
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(command: &str, show_terminal: bool, suppress_default: bool) -> GuiHook {
        GuiHook {
            enabled: true,
            command: command.to_string(),
            show_terminal,
            suppress_default,
        }
    }

    #[test]
    fn events_parse_from_cli_names_and_settings_keys() {
        for event in GuiHookEvent::ALL {
            assert_eq!(GuiHookEvent::parse(event.as_str()), Some(event));
            assert_eq!(GuiHookEvent::parse(event.config_key()), Some(event));
        }
        assert_eq!(GuiHookEvent::parse("on-tab-create"), None);
    }

    #[test]
    fn hooks_round_trip_through_settings_json() {
        let config = GuiHooksConfig {
            on_workspace_create: hook("~/bin/new.sh --force", true, false),
            on_workspace_delete: GuiHook {
                enabled: false,
                ..hook("rm-ws -y", false, true)
            },
        };
        let value = config.to_value().expect("hooks are saved");
        assert_eq!(
            value,
            json!({
                "on_workspace_create": {"enabled": true, "command": "~/bin/new.sh --force", "show_terminal": true, "suppress_default": false},
                "on_workspace_delete": {"enabled": false, "command": "rm-ws -y", "show_terminal": false, "suppress_default": true},
            })
        );
        assert_eq!(GuiHooksConfig::from_value(Some(&value)), config);
    }

    #[test]
    fn unset_hooks_are_not_saved_and_bad_entries_are_ignored() {
        assert_eq!(GuiHooksConfig::default().to_value(), None);
        let config = GuiHooksConfig::from_value(Some(&json!({
            "on_workspace_create": {"show_terminal": true},
            "on_workspace_delete": "echo",
        })));
        assert_eq!(config, GuiHooksConfig::default());
        assert_eq!(config.active(GuiHookEvent::WorkspaceCreate), None);
    }

    #[test]
    fn hooks_saved_without_the_enabled_flag_are_enabled() {
        let config = GuiHooksConfig::from_value(Some(&json!({
            "on_workspace_create": {"command": "new.sh", "show_terminal": true},
        })));
        assert_eq!(config.on_workspace_create, hook("new.sh", true, false));
        assert!(config.active(GuiHookEvent::WorkspaceCreate).is_some());
    }

    #[test]
    fn a_disabled_hook_keeps_its_settings_but_does_not_run() {
        let mut config = GuiHooksConfig {
            on_workspace_create: hook("new.sh", true, true),
            ..Default::default()
        };
        GuiHookUpdate {
            enabled: Some(false),
            ..Default::default()
        }
        .apply(config.get_mut(GuiHookEvent::WorkspaceCreate));
        assert_eq!(config.active(GuiHookEvent::WorkspaceCreate), None);
        assert_eq!(
            config.describe(GuiHookEvent::WorkspaceCreate),
            json!({
                "event": "on-workspace-create",
                "configured": true,
                "enabled": false,
                "command": "new.sh",
                "show_terminal": true,
                "suppress_default": true,
            })
        );
        // A disabled hook is not a default one, so it is still saved.
        let saved = GuiHooksConfig::from_value(config.to_value().as_ref());
        assert_eq!(saved, config);
    }

    #[test]
    fn a_blank_command_is_not_configured() {
        let config = GuiHooksConfig {
            on_workspace_delete: hook("   ", true, true),
            ..Default::default()
        };
        assert_eq!(config.active(GuiHookEvent::WorkspaceDelete), None);
    }

    #[test]
    fn updates_change_only_the_given_fields() {
        let mut current = hook("old", true, false);
        GuiHookUpdate {
            suppress_default: Some(true),
            ..Default::default()
        }
        .apply(&mut current);
        assert_eq!(current, hook("old", true, true));
        GuiHookUpdate {
            command: Some("  new --force ".to_string()),
            show_terminal: Some(false),
            ..Default::default()
        }
        .apply(&mut current);
        assert_eq!(current, hook("new --force", false, true));
    }

    #[test]
    fn environment_describes_the_event() {
        let env = hook_environment(
            GuiHookEvent::WorkspaceDelete,
            &hook("x", false, true),
            &GuiHookContext {
                workspace_id: Some("ws-1".to_string()),
                workspace_name: Some("api".to_string()),
                folder: Some("/src/api".to_string()),
                group_id: None,
            },
        );
        let pairs: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("LIMUX_HOOK", "on-workspace-delete"),
                ("LIMUX_HOOK_DEFAULT_ACTION", "suppressed"),
                ("LIMUX_WORKSPACE_ID", "ws-1"),
                ("LIMUX_WORKSPACE_NAME", "api"),
                ("LIMUX_WORKSPACE_FOLDER", "/src/api"),
            ]
        );
    }

    #[test]
    fn terminal_command_runs_the_hook_in_bash_and_waits() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let command = terminal_command(
            "echo \"it's $LIMUX_PANE_ID\" --force; exit_code() { return 3; }; exit_code",
            &["LIMUX_PANE_ID"],
        );
        // Run it the way Ghostty does, and press Enter at the closing prompt.
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .env("LIMUX_PANE_ID", "inherited")
            .env("LIMUX_HOOK", "on-workspace-create")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh runs");
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        let output = child.wait_with_output().expect("hook finishes");
        assert!(output.status.success(), "{command}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "it's  --force\n\n[on-workspace-create exited with status 3. Press Enter to close.]"
        );
    }
}
